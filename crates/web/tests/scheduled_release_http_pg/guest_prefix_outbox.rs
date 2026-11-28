//! AUTH-READ-02c2a: the real G claim/ack path and exact B write fence.
use super::pg;
use std::time::{SystemTime, UNIX_EPOCH};
use tor_books::{
    repositories::{
        guest_prefix::{
            GuestPrefixPreparation, GuestPrefixTarget, PgGuestPrefix, GUEST_PREFIX_CONSUMER,
            GUEST_PREFIX_EVENT,
        },
        postgres::PgAuthoring,
        PgCatalog,
    },
    services::authoring::{save_chapter, AuthorError, SaveChapterRequest, SavedChapter},
};
use tor_ingest::repositories::outbox::{OutboxError, PgOutbox};
use tor_web::guest_prefix_worker::{
    prepare_legacy_job, GuestPrefixConsumer, GuestPrefixWorkerOutcome,
};

pub(super) async fn fixture() -> (String, PgCatalog, i64, i64) {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PG CI");
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let owner: i64 = pg(
        &url,
        &format!(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES('auth-read-outbox-{nonce}@example.test','Guest prefix CI','$argon2id$fixture')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,
                               owner_user_id,access_level,guest_policy_revision)
             VALUES('ci:auth-read-outbox:{nonce}','ru','Guest prefix queue','public',
                    {owner},'basic',1) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    (url, catalog, owner, book)
}

pub(super) async fn publish(
    author: &PgAuthoring,
    owner: i64,
    book: i64,
    number: i32,
    key: char,
    source: &str,
) -> SavedChapter {
    author
        .create_published_free_chapter(
            owner,
            &save_chapter(SaveChapterRequest {
                book_id: book as u64,
                chapter_id: None,
                number,
                expected_revision: 0,
                title: "Guest prefix outbox",
                body: source,
                access: "free",
                price_amount: 0,
                command_key: &key.to_string().repeat(43),
            })
            .unwrap(),
        )
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn auth_read_real_outbox_claim_replay_and_stale_epoch() {
    let (url, catalog, owner, book) = fixture().await;
    let pool = catalog.pool();
    let author = PgAuthoring::new(pool.clone());
    let prefix = PgGuestPrefix::new(pool.clone());
    let outbox = PgOutbox::new(pool);
    let source = "Visible beginning. The private tail belongs only to the full chapter.";
    let saved = publish(&author, owner, book, 7, 'a', source).await;
    let page = prefix.enqueue_missing_page(book, 1, 0, 64).await.unwrap();
    assert_eq!(page.enqueued, 1);
    assert_eq!(page.next_after_chapter_no, None);
    assert_eq!(
        prefix
            .enqueue_missing_page(book, 1, 0, 64)
            .await
            .unwrap()
            .enqueued,
        0
    );
    let first = outbox
        .claim_next_for_consumer(GUEST_PREFIX_EVENT, GUEST_PREFIX_CONSUMER)
        .await
        .unwrap()
        .unwrap();
    let job = prefix
        .claimed_job(&first.id, first.lease_epoch)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        job.target(),
        GuestPrefixTarget {
            book_id: book,
            chapter_id: saved.chapter_id,
            version_id: saved.version_id,
            policy_revision: 1,
        }
    );
    let payload = pg(
        &url,
        &format!(
            "SELECT payload::text FROM outbox_events WHERE id='{}'::uuid",
            first.id
        ),
    );
    assert!(!payload.contains("private tail"));
    let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(payload.as_object().unwrap().len(), 6);
    assert_eq!(payload["source_sha256"].as_str().unwrap().len(), 64);
    assert_eq!(
        prefix.prepare_claimed(&job, "wrong source").await,
        Err(AuthorError::Conflict)
    );
    // Model a worker whose verified GET completed after its lease expired.
    pg(
        &url,
        &format!(
            "UPDATE outbox_events SET lease_until=clock_timestamp()-interval '1 second'
             WHERE id='{}'::uuid",
            first.id
        ),
    );
    let second = outbox
        .claim_next_for_consumer(GUEST_PREFIX_EVENT, GUEST_PREFIX_CONSUMER)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.id, second.id);
    assert_eq!(second.lease_epoch, first.lease_epoch + 1);
    assert!(prefix
        .claimed_job(&first.id, first.lease_epoch)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        prefix.prepare_claimed(&job, source).await,
        Err(AuthorError::Conflict)
    );
    assert_eq!(prepare_legacy_job(&prefix, &job).await, Ok(None));
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_version_guest_prefixes WHERE version_id={}",
                saved.version_id
            )
        ),
        "0"
    );
    assert_eq!(
        outbox
            .acknowledge(&first.id, first.lease_epoch, GUEST_PREFIX_CONSUMER)
            .await,
        Err(OutboxError::LostLease)
    );
    let job = prefix
        .claimed_job(&second.id, second.lease_epoch)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        prepare_legacy_job(&prefix, &job).await,
        Ok(Some(GuestPrefixPreparation::Stored))
    );
    assert_eq!(
        prepare_legacy_job(&prefix, &job).await,
        Ok(Some(GuestPrefixPreparation::AlreadyStored))
    );
    assert!(outbox
        .acknowledge(&second.id, second.lease_epoch, GUEST_PREFIX_CONSUMER)
        .await
        .unwrap());
    assert_eq!(
        prefix.prepare_claimed(&job, source).await,
        Err(AuthorError::Conflict)
    );
    assert!(outbox
        .claim_next_for_consumer(GUEST_PREFIX_EVENT, GUEST_PREFIX_CONSUMER)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        prefix
            .enqueue_missing_page(book, 1, 0, 64)
            .await
            .unwrap()
            .enqueued,
        0
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_version_guest_prefixes WHERE version_id={}",
                saved.version_id
            )
        ),
        "1"
    );
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn auth_read_consumer_retries_holds_and_settles_only_proved_outcomes() {
    let (url, catalog, owner, book) = fixture().await;
    let author = PgAuthoring::new(catalog.pool().clone());
    let prefix = PgGuestPrefix::new(catalog.pool().clone());
    let worker = GuestPrefixConsumer::new(&catalog);
    let source = "Visible beginning. The private tail belongs only to the full chapter.";
    let saved = publish(&author, owner, book, 7, 'a', source).await;
    assert_eq!(
        prefix
            .enqueue_missing_page(book, 1, 0, 64)
            .await
            .unwrap()
            .enqueued,
        1
    );
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=TRUE WHERE id={book}"),
    );
    assert_eq!(
        worker.run_one_legacy().await.unwrap(),
        GuestPrefixWorkerOutcome::RetryScheduled
    );
    let queued = |chapter: i64| {
        pg(
            &url,
            &format!(
                "SELECT attempts::text || ':' || (delivered_at IS NOT NULL)::text
                         || ':' || (dead_at IS NOT NULL)::text
                         || ':' || (lease_until IS NULL)::text
                 FROM outbox_events WHERE event_type='GuestPrefixPrepare'
                   AND aggregate_id={chapter} ORDER BY created_at,id LIMIT 1"
            ),
        )
    };
    assert_eq!(queued(saved.chapter_id), "1:false:false:true");
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_version_guest_prefixes WHERE version_id={}",
                saved.version_id
            )
        ),
        "0"
    );
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=FALSE WHERE id={book}"),
    );
    pg(
        &url,
        &format!(
            "UPDATE outbox_events SET next_attempt_at=clock_timestamp()
             WHERE event_type='GuestPrefixPrepare' AND aggregate_id={}",
            saved.chapter_id
        ),
    );
    assert_eq!(
        worker.run_one_legacy().await.unwrap(),
        GuestPrefixWorkerOutcome::Prepared(GuestPrefixPreparation::Stored)
    );
    assert_eq!(queued(saved.chapter_id), "2:true:false:true");
    assert_eq!(
        worker.run_one_legacy().await.unwrap(),
        GuestPrefixWorkerOutcome::Idle
    );

    let next = publish(&author, owner, book, 8, 'b', source).await;
    assert_eq!(
        prefix
            .enqueue_missing_page(book, 1, 0, 64)
            .await
            .unwrap()
            .enqueued,
        1
    );
    pg(
        &url,
        &format!("UPDATE books SET guest_policy_revision=2 WHERE id={book}"),
    );
    assert_eq!(
        worker.run_one_legacy().await.unwrap(),
        GuestPrefixWorkerOutcome::Superseded
    );
    assert_eq!(queued(next.chapter_id), "1:true:false:true");
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_version_guest_prefixes WHERE version_id={}",
                next.version_id
            )
        ),
        "0"
    );
    assert_eq!(
        prefix
            .enqueue_missing_page(book, 2, 0, 64)
            .await
            .unwrap()
            .enqueued,
        1
    );
    assert_eq!(
        worker.run_one_legacy().await.unwrap(),
        GuestPrefixWorkerOutcome::Prepared(GuestPrefixPreparation::Stored)
    );

    let invalid = pg(
        &url,
        &format!(
            "INSERT INTO outbox_events(event_type,schema_version,aggregate_kind,
                                       aggregate_id,dedupe_key,payload,required_consumers)
             VALUES('GuestPrefixPrepare',1,'chapter',{},'ci:auth-read-invalid:{book}',
                    jsonb_build_object('bad','payload'),ARRAY['guest_prefix_prepare_v1'])
             RETURNING id::text",
            next.chapter_id
        ),
    );
    assert_eq!(
        worker.run_one_legacy().await.unwrap(),
        GuestPrefixWorkerOutcome::DeadLetter
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT (dead_at IS NOT NULL AND delivered_at IS NULL
                          AND lease_until IS NULL)::text
                 FROM outbox_events WHERE id='{invalid}'::uuid"
            ),
        ),
        "true"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM event_deliveries WHERE event_id='{invalid}'::uuid"),
        ),
        "0"
    );
    assert_eq!(
        worker.run_one_legacy().await.unwrap(),
        GuestPrefixWorkerOutcome::Idle
    );
}
