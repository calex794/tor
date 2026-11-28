//! D-001 / D-06: Direct Swap versus actual paid V2 attachment.
use super::*;
use tor_commerce::repositories::{
    direct_swaps::{PgDirectSwaps, SwapError, SwapReceipt},
    swap_intent::DirectSwapIntent,
};

fn offer(url: &str, book: i64) -> (ChapterVersionRef, i64) {
    let raw = pg(
        url,
        &format!(
            "SELECT json_build_array(c.id,p.version_id,c.edit_revision)
             FROM chapters c JOIN chapter_publications p ON p.chapter_id=c.id
             WHERE c.book_id={book}"
        ),
    );
    let (chapter, version, revision): (i64, i64, i64) = serde_json::from_str(&raw).unwrap();
    (
        ChapterVersionRef {
            chapter_id: PositiveId::new(chapter).unwrap(),
            version_id: PositiveId::new(version).unwrap(),
        },
        revision,
    )
}

#[tokio::test]
#[ignore = "requires disposable PG18 and isolated MinIO"]
async fn d001_paid_author_first_rejects_stale_direct_swap_without_financial_changes() {
    swap_attachment_race(true).await;
}

#[tokio::test]
#[ignore = "requires disposable PG18 and isolated MinIO"]
async fn d001_paid_swap_first_pins_v1_before_author_publishes_v2() {
    swap_attachment_race(false).await;
}

async fn swap_attachment_race(author_first: bool) {
    let url = env::var("TEST_DATABASE_URL").expect("CI isolated database");
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("@127.0.0.1:5432/tor_ci"));
    assert_eq!(env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
    assert_eq!(env::var("TOR_TEST_S3_ENDPOINT").as_deref(), Ok(ENDPOINT));
    let case = if author_first { "author" } else { "swap" };
    let bucket = format!("tor-d001-swap-{case}-ci");
    let client = Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new(
                env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
                env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
                None,
                None,
                "local-d001-swap-attachment",
            ))
            .endpoint_url(ENDPOINT)
            .force_path_style(true)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .build(),
    );
    client.create_bucket().bucket(&bucket).send().await.unwrap();
    let store = S3PrivateStore::new(client, bucket.clone()).unwrap();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let (owner, _) = user(
        &accounts,
        &format!("d001-swap-{case}-owner@example.invalid"),
    )
    .await;
    let (buyer, _) = user(
        &accounts,
        &format!("d001-swap-{case}-buyer@example.invalid"),
    )
    .await;
    let ring = Arc::new(
        ContentKeyring::with_v2(
            Arc::new(ContentKek::from_bytes([0x73; 32])),
            Arc::new(ContentKek::from_bytes([0x74; 32])),
        )
        .unwrap()
        .with_active_id("content-kek-v2")
        .unwrap(),
    );
    let writer =
        PgPaidAuthorPublisher::new(catalog, store.clone(), ring.clone(), bucket.clone()).unwrap();
    let policy = format!("ci:d001:swap:legal:{buyer}");
    let terms = format!("ci:d001:swap:terms:{buyer}");
    pg(
        &url,
        &format!(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
             VALUES({buyer},1000,'RUB');
             INSERT INTO finance_direct_refund_policy_catalog
                 (version,approval_reference,approved_by_user_id,approved_at,effective_at,window_seconds)
             VALUES('{policy}','ci:d001:swap:approval:{buyer}',{owner},
                 now()-interval '2 days',now()-interval '1 day',86400)"
        ),
    );
    let mut books = Vec::new();
    for (label, price, body) in [
        ("source", 300, "Original source purchase"),
        ("target", 450, "Target bytes V1"),
    ] {
        let book: i64 = pg(
            &url,
            &format!(
                "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
                 VALUES('ci:d001:swap:{case}:{label}','ru','D001 swap {label}','public',{owner},'basic')
                 RETURNING id"
            ),
        )
        .parse()
        .unwrap();
        pg(
            &url,
            &format!(
                "INSERT INTO finance_revenue_terms
                  (book_id,policy_version,platform_fee_bps,refund_policy_version,refund_window_seconds)
                 VALUES({book},'{terms}',2000,'{policy}',86400);
                 INSERT INTO finance_revenue_term_states(book_id,policy_version)
                 VALUES({book},'{terms}')"
            ),
        );
        let initial = save_chapter(SaveChapterRequest {
            book_id: book as u64,
            chapter_id: None,
            number: 1,
            expected_revision: 0,
            title: "D001 swap edition",
            body,
            access: "paid",
            price_amount: price,
            command_key: &generate_session_token(),
        })
        .unwrap();
        writer.publish_bodyless(owner, &initial).await.unwrap();
        books.push((book, offer(&url, book)));
    }
    let [(_, (old, _)), (target_book, (v1, revision))] = books[..] else {
        panic!("requires source and target offers");
    };
    let actor = PositiveId::new(buyer).unwrap();
    let cmd = PurchaseCommand::new(
        actor,
        WalletId::new(buyer).unwrap(),
        old,
        MoneyMinor::new(300).unwrap(),
        IdempotencyKey::parse(&generate_session_token()).unwrap(),
    )
    .unwrap();
    let source = PgCheckout::new(pool.clone())
        .with_creator_posting(true)
        .checkout_once(&cmd)
        .await
        .unwrap()
        .purchase_id
        .get();
    let chapter = v1.chapter_id.get();
    let version = v1.version_id.get();
    let intent = DirectSwapIntent {
        buyer_id: buyer,
        source_purchase_id: source,
        target_chapter_id: chapter,
        target_version_id: version,
        source_price_minor: 300,
        target_price_minor: 450,
    };
    let key = IdempotencyKey::parse(&generate_session_token()).unwrap();
    let paid = PgPaidStorage::new(pool.clone());
    let old_target = paid.encrypted_chapter(v1).await.unwrap().unwrap();
    let v1_ciphertext = store.get_verified(&old_target.object).await.unwrap();
    let edit = save_chapter(SaveChapterRequest {
        book_id: target_book as u64,
        chapter_id: Some(chapter as u64),
        number: 1,
        expected_revision: revision,
        title: "D001 swap edition",
        body: "Verified target replacement V2",
        access: "paid",
        price_amount: 450,
        command_key: &generate_session_token(),
    })
    .unwrap();
    let prepared = paid
        .prepare_published_paid_edit_bodyless(owner, &edit)
        .await
        .unwrap();
    assert!(!prepared.published);
    assert_eq!(prepared.previous_version_id, version);
    let v2_id = prepared.version_id;
    let v2 = ChapterVersionRef {
        chapter_id: v1.chapter_id,
        version_id: PositiveId::new(v2_id).unwrap(),
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
    let attach = paid.attach_verified_and_publish_bodyless(
        owner,
        target_book,
        v2,
        &object_id,
        PaidPublication::PublishEdit {
            expected_current: prepared.previous_version_id,
            expected_revision: prepared.expected_revision,
        },
        &edit.body,
    );
    let swaps = PgDirectSwaps::new(pool.clone());
    let swap = swaps.swap_once(intent, &key);
    let ledger = || {
        pg(
            &url,
            &format!(
                "SELECT balance_minor,
                    (SELECT count(*) FROM finance_direct_swap_commands WHERE buyer_user_id={buyer}),
                    (SELECT count(*) FROM finance_direct_swap_postings WHERE source_purchase_id={source}),
                    (SELECT count(*) FROM chapter_purchase_revocations WHERE purchase_id={source}),
                    (SELECT count(*) FROM chapter_purchase_facts WHERE user_id={buyer} AND chapter_id={chapter}),
                    (SELECT count(*) FROM reader_wallet_entries WHERE user_id={buyer})
                 FROM reader_wallet_accounts WHERE user_id={buyer}"
            ),
        )
    };
    assert_eq!(ledger(), "700|0|0|0|0|1");
    let source_chapter = old.chapter_id.get();
    let source_version = old.version_id.get();
    let source_readable = || {
        pg(&url, &format!(
            "SELECT tor_purchase_version_readable({buyer},{source},{source_chapter},{source_version})"
        ))
    };
    assert_eq!(source_readable(), "t");

    if author_first {
        let (gate, gate_pid) = RowGate::hold(
            &url,
            &format!("SELECT id FROM chapter_versions WHERE id={v2_id} FOR UPDATE"),
            &v2_id.to_string(),
        );
        let (published, result) = tokio::time::timeout(Duration::from_secs(35), async {
            tokio::join!(attach, async {
                let author_pid = blocked_statement(
                    &url,
                    gate_pid,
                    &["SELECT storage_object_id::text,body_markdown,content_sha256"],
                )
                .await;
                let (result, ()) = tokio::join!(swap, async {
                    let swap_pid = blocked_statement(
                        &url,
                        author_pid,
                        &["SELECT id,owner_user_id FROM books"],
                    )
                    .await;
                    assert_ne!(swap_pid, gate_pid);
                    gate.release();
                });
                result
            })
        })
        .await
        .expect("both real transactions must terminate");
        published.expect("V2 must attach");
        assert_eq!(result, Err(SwapError::Changed));
        assert_eq!(ledger(), "700|0|0|0|0|1");
        assert_eq!(source_readable(), "t");
    } else {
        // This lock lets the command's wallet FK take KEY SHARE, but stops
        // settlement only after the swap holds both book locks and the V1 offer.
        let (gate, gate_pid) = RowGate::hold(
            &url,
            &format!(
                "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={buyer} FOR NO KEY UPDATE"
            ),
            "700",
        );
        let (result, ()) = tokio::time::timeout(Duration::from_secs(35), async {
            tokio::join!(swap, async {
                let swap_pid = blocked_statement(
                    &url,
                    gate_pid,
                    &["SELECT balance_minor FROM reader_wallet_accounts"],
                )
                .await;
                let (published, ()) = tokio::join!(attach, async {
                    let author_pid =
                        blocked_statement(&url, swap_pid, &["SELECT visibility FROM books"]).await;
                    assert_ne!(author_pid, gate_pid);
                    assert_eq!(offer(&url, target_book).0, v1);
                    gate.release();
                });
                published.expect("V2 must attach after the swap commits");
            })
        })
        .await
        .expect("both real transactions must terminate");
        let first = result.expect("swap of still-current V1");
        assert!(!first.replayed);
        assert_eq!(
            (first.old_purchase, first.delta_minor, first.balance_after),
            (source, 150, 550)
        );
        let purchase = first.new_purchase;
        let exact_entitlement = pg(
            &url,
            &format!(
                "SELECT f.version_id, f.amount_minor,
                    tor_purchase_version_readable({buyer},{purchase},{chapter},{version}),
                    tor_purchase_version_readable({buyer},{purchase},{chapter},{v2_id}),
                    (SELECT count(*) FROM reader_wallet_entries
                     WHERE purchase_id={source} AND kind='chapter_swap_credit'),
                    (SELECT count(*) FROM reader_wallet_entries
                     WHERE purchase_id={purchase} AND kind='chapter_purchase'),
                    (SELECT platform_reversal_minor FROM finance_direct_swap_postings
                     WHERE source_purchase_id={source} AND new_purchase_id={purchase}),
                    (SELECT COALESCE(sum(amount_minor),0)::BIGINT
                     FROM finance_direct_swap_creator_reversals WHERE source_purchase_id={source})
                 FROM chapter_purchase_facts f
                 WHERE f.id={purchase}"
            ),
        );
        assert_eq!(exact_entitlement, format!("{version}|450|t|f|1|1|60|240"));
        assert_eq!(source_readable(), "f");
        let replay = swaps.swap_once(intent, &key).await.unwrap();
        assert!(replay.replayed);
        assert_eq!(
            replay,
            SwapReceipt {
                replayed: true,
                ..first
            }
        );
        assert_eq!(ledger(), "550|1|1|1|1|3");
    }
    assert_eq!(offer(&url, target_book), (v2, revision + 1));
    assert_eq!(
        store.get_verified(&old_target.object).await.unwrap(),
        v1_ciphertext
    );
    let current = registry
        .owned_current_paid_markdown(owner, target_book, v2, &bucket, &store, &ring)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.as_str(), edit.body.as_str());
    writeln!(
        std::io::stdout().lock(),
        "D001_DIRECT_SWAP_AUTHOR_ATTACHMENT_OK: author_first={author_first}"
    )
    .unwrap();
}
