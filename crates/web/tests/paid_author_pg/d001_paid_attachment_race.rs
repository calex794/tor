//! D-001: prove checkout fences the final verified paid publication transaction,
//! not merely the earlier author preparation transaction.
use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Stdio},
};
use tor_books::repositories::paid_storage::PaidPublication;
use tor_commerce::{
    domain::money::{
        CheckoutError, CheckoutRepository, IdempotencyKey, MoneyMinor, PurchaseCommand, WalletId,
    },
    repositories::checkout::PgCheckout,
};
use tor_storage::{
    repositories::registry::PgStorageRegistry,
    services::{
        crypto::{fresh_private_object_id, seal_chapter_with_keyring},
        provider::upload_and_verify,
    },
};

#[path = "d001_paid_swap_attachment_race.rs"]
mod d001_paid_swap_attachment_race;

// Keep one disposable psql session alive to hold a fixture row. The web
// crate deliberately does not depend on the PostgreSQL client library.
struct RowGate(Child);

impl RowGate {
    fn hold(url: &str, lock_sql: &str, expected: &str) -> (Self, i32) {
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
                .expect("isolated Linux CI psql lock session"),
        );
        write!(
            gate.0.stdin.as_mut().unwrap(),
            "BEGIN;\nSET LOCAL statement_timeout='5s';\nSELECT pg_backend_pid();\n\
             {lock_sql};\n"
        )
        .unwrap();
        let mut out = BufReader::new(gate.0.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).unwrap();
        let pid = line.trim().parse().expect("lock session PID");
        line.clear();
        out.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), expected);
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

impl Drop for RowGate {
    fn drop(&mut self) {
        if matches!(self.0.try_wait(), Ok(None)) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

async fn blocked_statement(url: &str, blocker: i32, prefixes: &[&str]) -> i32 {
    let patterns = prefixes
        .iter()
        .map(|prefix| format!("'{prefix}%'"))
        .collect::<Vec<_>>()
        .join(",");
    tokio::time::timeout(Duration::from_secs(12), async {
        loop {
            let result = pg(
                url,
                &format!(
                    "SELECT pid FROM pg_stat_activity
                     WHERE pid<>{blocker} AND {blocker}=ANY(pg_blocking_pids(pid))
                     AND state='active' AND query LIKE ANY(ARRAY[{patterns}])
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
    .unwrap_or_else(|_| panic!("no PostgreSQL blocker {blocker} for {prefixes:?}"))
}

#[tokio::test]
#[ignore = "requires disposable PG18 and isolated MinIO"]
async fn d001_paid_checkout_fences_final_verified_author_attachment() {
    run_paid_attachment_race(false).await;
}

#[tokio::test]
#[ignore = "requires disposable PG18 and isolated MinIO"]
async fn d001_paid_final_author_attachment_rejects_stale_checkout_without_debit() {
    run_paid_attachment_race(true).await;
}

async fn run_paid_attachment_race(author_first: bool) {
    let url = env::var("TEST_DATABASE_URL").expect("CI isolated database");
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("@127.0.0.1:5432/tor_ci"));
    assert_eq!(env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(env::var("TOR_TEST_S3_ENDPOINT").as_deref(), Ok(ENDPOINT));
    let case = if author_first { "author" } else { "buyer" };
    let bucket = format!("tor-private-d001-attach-{case}-ci");

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
    client.create_bucket().bucket(&bucket).send().await.unwrap();
    let store = S3PrivateStore::new(client, bucket.clone()).unwrap();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let (owner, _) = user(
        &accounts,
        &format!("d001-attach-{case}-owner@example.invalid"),
    )
    .await;
    let (buyer, _) = user(
        &accounts,
        &format!("d001-attach-{case}-buyer@example.invalid"),
    )
    .await;
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
             VALUES('ci:d001-attach-race:{case}','ru','D001 attachment race','public',{owner},'basic')
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
    let writer =
        PgPaidAuthorPublisher::new(catalog, store.clone(), ring.clone(), bucket.clone()).unwrap();
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
    let ids: Vec<i64> = initial_ids
        .split('|')
        .map(|id| id.parse().unwrap())
        .collect();
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
        .reserve_pending_with_keyring(&sealed, &ring, &bucket)
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
    let cmd = PurchaseCommand::new(
        PositiveId::new(buyer).unwrap(),
        WalletId::new(buyer).unwrap(),
        v1,
        MoneyMinor::new(225).unwrap(),
        IdempotencyKey::parse(&generate_session_token()).unwrap(),
    )
    .unwrap();
    let checkout = PgCheckout::new(pool.clone());
    let attach = paid.attach_verified_and_publish_bodyless(
        owner,
        book,
        v2,
        &object_id,
        PaidPublication::PublishEdit {
            expected_current: prepared.previous_version_id,
            expected_revision: prepared.expected_revision,
        },
        &edit.body,
    );
    if author_first {
        let v2_id = v2.version_id.get();
        // Hold only the prepared V2 row. This lets final attachment acquire
        // Book and Chapter before it waits, without locking the offered V1.
        let (row, row_pid) = RowGate::hold(
            &url,
            &format!("SELECT id FROM chapter_versions WHERE id={v2_id} FOR UPDATE"),
            &v2_id.to_string(),
        );
        let (published, purchase) = tokio::time::timeout(Duration::from_secs(35), async {
            tokio::join!(attach, async {
                let author_pid = blocked_statement(
                    &url,
                    row_pid,
                    &["SELECT storage_object_id::text,body_markdown,content_sha256"],
                )
                .await;
                let (purchase, ()) = tokio::join!(checkout.checkout_once(&cmd), async {
                    // Checkout must wait at the Book fence, before the
                    // command's FK can lock the chapter or expected edition.
                    let buyer_pid =
                        blocked_statement(&url, author_pid, &["SELECT b.id FROM books b"]).await;
                    assert_ne!(buyer_pid, row_pid);
                    row.release();
                });
                purchase
            })
        })
        .await
        .expect("stale checkout and final publication must both terminate");
        published.expect("verified V2 must become current");
        assert_eq!(purchase.unwrap_err(), CheckoutError::StaleOffer);
        let persisted = pg(
            &url,
            &format!(
                "SELECT w.balance_minor,
                   (SELECT count(*) FROM chapter_purchase_facts WHERE user_id={buyer}),
                   (SELECT count(*) FROM library_purchased_chapters WHERE user_id={buyer}),
                   (SELECT count(*) FROM reader_wallet_entries WHERE user_id={buyer}),
                   (SELECT count(*) FROM chapter_purchase_commands WHERE user_id={buyer})
                 FROM reader_wallet_accounts w WHERE w.user_id={buyer}"
            ),
        );
        assert_eq!(
            persisted, "600|0|0|0|0",
            "stale V1 must leave no charge or rights"
        );
    } else {
        let (wallet, wallet_pid) = RowGate::hold(
            &url,
            &format!(
                "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer} FOR UPDATE"
            ),
            "600",
        );
        let (purchase, ()) = tokio::time::timeout(Duration::from_secs(35), async {
            tokio::join!(checkout.checkout_once(&cmd), async {
                let buyer_pid = blocked_statement(
                    &url,
                    wallet_pid,
                    &["SELECT balance_minor FROM reader_wallet_accounts"],
                )
                .await;
                let (published, ()) = tokio::join!(attach, async {
                    let editor_pid =
                        blocked_statement(&url, buyer_pid, &["SELECT visibility FROM books"])
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
                });
                published.expect("buyer COMMIT must unblock final author attachment");
            })
        })
        .await
        .expect("checkout and final author attachment must both terminate");
        assert_eq!(purchase.expect("checkout of still-current V1").pinned, v1);

        let persisted = pg(
            &url,
            &format!(
                "SELECT w.balance_minor,count(f.id),min(f.version_id)
                 FROM reader_wallet_accounts w
                 JOIN chapter_purchase_facts f ON f.user_id=w.user_id
                 WHERE w.user_id={buyer} AND f.chapter_id={chapter} GROUP BY w.balance_minor"
            ),
        );
        assert_eq!(persisted, format!("375|1|{version}"));
    }
    assert_eq!(
        pg(
            &url,
            &format!("SELECT version_id FROM chapter_publications WHERE chapter_id={chapter}"),
        ),
        v2.version_id.get().to_string()
    );
    assert_eq!(
        store.get_verified(&old_object.object).await.unwrap(),
        old_ciphertext
    );
    let source = registry
        .owned_current_paid_markdown(owner, book, v2, &bucket, &store, &ring)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(source.as_str(), edit.body.as_str());
    let marker = if author_first {
        "D001_PAID_FINAL_AUTHOR_ATTACHMENT_REJECTS_STALE_CHECKOUT_WITHOUT_DEBIT_OK"
    } else {
        "D001_PAID_CHECKOUT_FENCES_FINAL_VERIFIED_ATTACHMENT_OK"
    };
    writeln!(std::io::stdout().lock(), "{marker}").unwrap();
    if author_first {
        return;
    }

    // Continue the real buyer-first case through T5. Replacing an unpinned
    // head adds an immutable object, not a fourth retained logical edition.
    let counts = || {
        pg(
            &url,
            &format!(
                "SELECT logical_editions, reader_retained_editions, physical_materializations
                 FROM d001_chapter_logical_counts WHERE chapter_id={chapter}"
            ),
        )
    };
    assert_eq!(counts(), "2|2|2");
    let make_edit = |expected_revision, body| {
        save_chapter(SaveChapterRequest {
            book_id: book as u64,
            chapter_id: Some(chapter as u64),
            number: 1,
            expected_revision,
            title: "D001 attachment race",
            body,
            access: "paid",
            price_amount: 225,
            command_key: &generate_session_token(),
        })
        .unwrap()
    };
    let unpinned_replacement = make_edit(revision + 1, "Unpinned logical V2 replacement");
    writer
        .publish_edit_bodyless(owner, &unpinned_replacement)
        .await
        .unwrap();
    assert_eq!(counts(), "2|2|3");
    let current = || {
        let id = pg(
            &url,
            &format!("SELECT version_id FROM chapter_publications WHERE chapter_id={chapter}"),
        )
        .parse()
        .unwrap();
        ChapterVersionRef {
            chapter_id: v1.chapter_id,
            version_id: PositiveId::new(id).unwrap(),
        }
    };
    let pinned_second = current();
    assert_ne!(pinned_second, v2);
    let second_object = paid
        .encrypted_chapter(pinned_second)
        .await
        .unwrap()
        .unwrap();
    let second_ciphertext = store.get_verified(&second_object.object).await.unwrap();
    let (second_buyer, _) = user(&accounts, "d001-attach-buyer-second@example.invalid").await;
    pg(
        &url,
        &format!(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
             VALUES({second_buyer},600,'RUB')"
        ),
    );
    let second_command = PurchaseCommand::new(
        PositiveId::new(second_buyer).unwrap(),
        WalletId::new(second_buyer).unwrap(),
        pinned_second,
        MoneyMinor::new(225).unwrap(),
        IdempotencyKey::parse(&generate_session_token()).unwrap(),
    )
    .unwrap();
    for _ in 0..2 {
        assert_eq!(
            checkout
                .checkout_once(&second_command)
                .await
                .unwrap()
                .pinned,
            pinned_second
        );
    }
    assert_eq!(counts(), "2|2|3");

    let final_edit = make_edit(revision + 2, "Current logical V3 after two paid pins");
    writer
        .publish_edit_bodyless(owner, &final_edit)
        .await
        .unwrap();
    let final_head = current();
    assert_ne!(final_head, pinned_second);
    assert_ne!(final_head, v1);
    assert_eq!(counts(), "3|3|4");
    for (reader, own_version, entity, expected_ciphertext) in [
        (buyer, v1, old_object, old_ciphertext),
        (
            second_buyer,
            pinned_second,
            second_object,
            second_ciphertext,
        ),
    ] {
        assert_eq!(
            pg(
                &url,
                &format!(
                    "SELECT w.balance_minor,count(f.id),min(f.version_id)
                     FROM reader_wallet_accounts w
                     JOIN chapter_purchase_facts f ON f.user_id=w.user_id
                     WHERE w.user_id={reader} AND f.chapter_id={chapter} GROUP BY w.balance_minor"
                ),
            ),
            format!("375|1|{}", own_version.version_id.get()),
        );
        assert_eq!(
            store.get_verified(&entity.object).await.unwrap(),
            expected_ciphertext
        );
    }
    let source = registry
        .owned_current_paid_markdown(owner, book, final_head, &bucket, &store, &ring)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(source.as_str(), final_edit.body.as_str());
    writeln!(
        std::io::stdout().lock(),
        "D001_TWO_PAID_PINS_THREE_LOGICAL_EDITIONS_OK"
    )
    .unwrap();
}
