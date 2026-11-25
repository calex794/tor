//! D-001: prove checkout fences the final verified paid publication transaction,
//! not merely the earlier author preparation transaction.
use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Stdio},
};
use tor_books::repositories::paid_storage::PaidPublication;
use tor_commerce::{
    domain::money::{CheckoutRepository, IdempotencyKey, MoneyMinor, PurchaseCommand, WalletId},
    repositories::checkout::PgCheckout,
};
use tor_storage::{
    repositories::registry::PgStorageRegistry,
    services::{
        crypto::{fresh_private_object_id, seal_chapter_with_keyring},
        provider::upload_and_verify,
    },
};

// Keep one disposable psql session alive to hold the wallet row. The web
// crate deliberately does not depend on the PostgreSQL client library.
struct WalletGate(Child);

impl WalletGate {
    fn hold(url: &str, buyer: i64) -> (Self, i32) {
        let mut gate = Self(
            Command::new("stdbuf")
                .args([
                    "-oL",
                    "psql",
                    "-XqAt",
                    "-v",
                    "ON_ERROR_STOP=1",
                    "--dbname",
                    url,
                ])
                .env("PGCONNECT_TIMEOUT", "4")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .expect("isolated Linux CI psql wallet session"),
        );
        write!(
            gate.0.stdin.as_mut().unwrap(),
            "BEGIN;\nSET LOCAL statement_timeout='5s';\nSELECT pg_backend_pid();\n\
             SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer} FOR UPDATE;\n"
        )
        .unwrap();
        let mut out = BufReader::new(gate.0.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        let pid = line.trim().parse().expect("wallet session PID");
        line.clear();
        out.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "600");
        gate.0.stdout = Some(out.into_inner());
        (gate, pid)
    }

    fn release(mut self) {
        let mut input = self.0.stdin.take().unwrap();
        input.write_all(b"COMMIT;\n\\q\n").unwrap();
        drop(input);
        assert!(self.0.wait().unwrap().success());
    }
}

impl Drop for WalletGate {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

async fn blocked_statement(url: &str, blocker: i32, prefix: &str) -> i32 {
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let result = pg(
                url,
                &format!(
                    "SELECT pid FROM pg_stat_activity
                     WHERE pid<>{blocker} AND {blocker}=ANY(pg_blocking_pids(pid))
                     AND state='active' AND query LIKE '{prefix}%'
                     ORDER BY pid LIMIT 1"
                ),
            );
            if !result.is_empty() {
                return result.parse().unwrap();
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no PostgreSQL blocker {blocker} for {prefix}"))
}

#[tokio::test]
#[ignore = "requires disposable PG18 and isolated MinIO"]
async fn d001_paid_checkout_fences_final_verified_author_attachment() {
    let url = env::var("TEST_DATABASE_URL").expect("CI isolated database");
    assert!(
        url.starts_with("postgresql://tor_app:") && url.ends_with("@127.0.0.1:5432/tor_ci")
    );
    assert_eq!(env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(env::var("TOR_TEST_S3_ENDPOINT").as_deref(), Ok(ENDPOINT));
    const BUCKET_RACE: &str = "tor-private-d001-attach-race-ci";

    let client = Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new(
                env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
                env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
                None,
                None,
                "local-d001-attach-race",
            ))
            .endpoint_url(ENDPOINT)
            .force_path_style(true)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .build(),
    );
    client
        .create_bucket()
        .bucket(BUCKET_RACE)
        .send()
        .await
        .unwrap();
    let store = S3PrivateStore::new(client, BUCKET_RACE.to_owned()).unwrap();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let (owner, _) = user(&accounts, "d001-attach-owner@example.invalid").await;
    let (buyer, _) = user(&accounts, "d001-attach-buyer@example.invalid").await;
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:d001-attach-race','ru','D001 attachment race','public',{owner},'basic')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let ring = Arc::new(
        ContentKeyring::with_v2(
            Arc::new(ContentKek::from_bytes([0x71; 32])),
            Arc::new(ContentKek::from_bytes([0x72; 32])),
        )
        .unwrap()
        .with_active_id("content-kek-v2")
        .unwrap(),
    );
    let writer = PgPaidAuthorPublisher::new(
        catalog,
        store.clone(),
        ring.clone(),
        BUCKET_RACE.to_owned(),
    )
    .unwrap();
    let initial = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: None,
        number: 1,
        expected_revision: 0,
        title: "D001 attachment race",
        body: "Original bought V1",
        access: "paid",
        price_amount: 225,
        command_key: &generate_session_token(),
    })
    .unwrap();
    writer.publish_bodyless(owner, &initial).await.unwrap();
    let initial_ids = pg(
        &url,
        &format!(
            "SELECT c.id,p.version_id,c.edit_revision
             FROM chapters c JOIN chapter_publications p ON p.chapter_id=c.id WHERE c.book_id={book}"
        ),
    );
    let ids: Vec<i64> = initial_ids.split('|').map(|id| id.parse().unwrap()).collect();
    let [chapter, version, revision] = ids[..] else {
        panic!("expected one published chapter with revision");
    };
    let v1 = ChapterVersionRef {
        chapter_id: PositiveId::new(chapter).unwrap(),
        version_id: PositiveId::new(version).unwrap(),
    };
    let paid = PgPaidStorage::new(pool.clone());
    let old_object = paid.encrypted_chapter(v1).await.unwrap().unwrap();
    let old_ciphertext = store.get_verified(&old_object.object).await.unwrap();

    // Stage and verify V2 fully before checkout starts. The editor future below
    // can only block inside final attachment, not author preparation or S3 I/O.
    let edit = save_chapter(SaveChapterRequest {
        book_id: book as u64,
        chapter_id: Some(chapter as u64),
        number: 1,
        expected_revision: revision,
        title: "D001 attachment race",
        body: "Verified replacement V2",
        access: "paid",
        price_amount: 225,
        command_key: &generate_session_token(),
    })
    .unwrap();
    let prepared = paid
        .prepare_published_paid_edit_bodyless(owner, &edit)
        .await
        .unwrap();
    assert!(!prepared.published);
    assert_eq!(prepared.previous_version_id, version);
    let v2 = ChapterVersionRef {
        chapter_id: v1.chapter_id,
        version_id: PositiveId::new(prepared.version_id).unwrap(),
    };
    let object_id = fresh_private_object_id().unwrap();
    let sealed = seal_chapter_with_keyring(v2, object_id.clone(), &edit.body, &ring).unwrap();
    let registry = PgStorageRegistry::new(pool.clone());
    let pending = registry
        .reserve_pending_with_keyring(&sealed, &ring, BUCKET_RACE)
        .await
        .unwrap();
    let verified = upload_and_verify(&store, &pending, &sealed.ciphertext)
        .await
        .unwrap();
    registry.mark_ready(&verified).await.unwrap();

    pg(
        &url,
        &format!(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
             VALUES({buyer},600,'RUB')"
        ),
    );
    let (wallet, wallet_pid) = WalletGate::hold(&url, buyer);

    let cmd = PurchaseCommand::new(
        PositiveId::new(buyer).unwrap(),
        WalletId::new(buyer).unwrap(),
        v1,
        MoneyMinor::new(225).unwrap(),
        IdempotencyKey::parse(&generate_session_token()).unwrap(),
    )
    .unwrap();
    let checkout = PgCheckout::new(pool.clone());
    let (purchase, ()) = tokio::time::timeout(Duration::from_secs(35), async {
        tokio::join!(checkout.checkout_once(&cmd), async {
            let buyer_pid = blocked_statement(
                &url,
                wallet_pid,
                "SELECT balance_minor FROM reader_wallet_accounts",
            )
            .await;
            let (published, ()) = tokio::join!(
                paid.attach_verified_and_publish_bodyless(
                    owner,
                    book,
                    v2,
                    &object_id,
                    PaidPublication::PublishEdit {
                        expected_current: prepared.previous_version_id,
                        expected_revision: prepared.expected_revision,
                    },
                    &edit.body,
                ),
                async {
                    let editor_pid =
                        blocked_statement(&url, buyer_pid, "SELECT visibility FROM books")
                            .await;
                    assert_ne!(editor_pid, wallet_pid);
                    assert_eq!(
                        pg(
                            &url,
                            &format!(
                                "SELECT version_id FROM chapter_publications WHERE chapter_id={chapter}"
                            ),
                        ),
                        version.to_string(),
                    );
                    wallet.release();
                },
            );
            published.expect("buyer COMMIT must unblock final author attachment");
        })
    })
    .await
    .expect("checkout and final author attachment must both terminate");
    assert_eq!(purchase.expect("checkout of still-current V1").pinned, v1);

    let persisted = pg(
        &url,
        &format!(
            "SELECT w.balance_minor,count(f.id),min(f.version_id),p.version_id
             FROM reader_wallet_accounts w
             JOIN chapter_purchase_facts f ON f.user_id=w.user_id
             JOIN chapter_publications p ON p.chapter_id=f.chapter_id
             WHERE w.user_id={buyer} AND f.chapter_id={chapter}
             GROUP BY w.balance_minor,p.version_id"
        ),
    );
    assert_eq!(
        persisted,
        format!("375|1|{version}|{}", v2.version_id.get())
    );
    assert_eq!(
        store.get_verified(&old_object.object).await.unwrap(),
        old_ciphertext
    );
    let source = registry
        .owned_current_paid_markdown(owner, book, v2, BUCKET_RACE, &store, &ring)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(source.as_str(), edit.body.as_str());
    std::io::Write::write_all(
        &mut std::io::stdout().lock(),
        b"D001_PAID_CHECKOUT_FENCES_FINAL_VERIFIED_ATTACHMENT_OK\n",
    )
    .unwrap();
}
