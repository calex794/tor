//! G-08 integration: owned outbox lease -> 201 durable metadata intents.
//! No email, push, paid content or false acknowledgements of search effects.
use sqlx::PgPool;
use tor_ingest::repositories::{
    notify::PgNotificationRecipients,
    outbox::PgOutbox,
};
use tor_ingest::services::notify_worker::{NotificationRun, NotificationWorker};

async fn database() -> PgPool {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL URL");
    assert!(url.ends_with("/tor_ci"), "never touch production");
    PgPool::connect(&url).await.unwrap()
}

async fn setup_book(pool: &PgPool, label: &str) -> (i64, i64, i64) {
    let author: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Notify author','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("notify-owner-{label}@example.invalid"))
    .fetch_one(pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','Notify book','public',$2) RETURNING id",
    )
    .bind(format!("ci:notify-worker:{label}"))
    .bind(author)
    .fetch_one(pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Notify chapter','public','free',0) RETURNING id",
    )
    .bind(book)
    .fetch_one(pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Public body stays outside event') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_first_release_events(chapter_id,version_id)
         VALUES($1,$2)",
    )
    .bind(chapter)
    .bind(version)
    .execute(pool)
    .await
    .unwrap();
    (book, chapter, version)
}

async fn event(
    pool: &PgPool,
    label: &str,
    book: i64,
    chapter: i64,
    version: i64,
) -> String {
    sqlx::query_scalar(
        "INSERT INTO outbox_events(event_type,schema_version,aggregate_kind,aggregate_id,
            dedupe_key,payload,required_consumers,next_attempt_at)
         VALUES('ChapterPublished',1,'chapter',$1,$2,
            jsonb_build_object('chapter_id',$1,'book_id',$3,'version_id',$4,
                'published_at',now(),'first_release',true),
            ARRAY['search-index-v1','subscription-inbox-v1']::text[],
            now()-interval '10 days')
         RETURNING id::text",
    )
    .bind(chapter)
    .bind(format!("notify-worker:{label}:{version}"))
    .bind(book)
    .bind(version)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PG18 /tor_ci"]
async fn g_08_worker_stages_all_201_without_acknowledging_search_or_sending() {
    let pool = database().await;
    let nonce: i64 = sqlx::query_scalar(
        "SELECT (EXTRACT(EPOCH FROM clock_timestamp()) * 1000000)::bigint",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let label = format!("{nonce}");
    let (book, chapter, version) = setup_book(&pool, &label).await;
    let prefix = format!("notify-worker-reader-{label}-");
    sqlx::query(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         SELECT $1 || seq::text || '@example.invalid',
                'Notify reader','$argon2id$fixture'
         FROM generate_series(1,201) AS batch(seq)",
    )
    .bind(&prefix)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_book_follows(user_id,book_id,created_at)
         SELECT id,$1,now()-interval '1 day'
         FROM users WHERE email_normalized LIKE $2",
    )
    .bind(book)
    .bind(format!("{prefix}%"))
    .execute(&pool)
    .await
    .unwrap();
    let event_id = event(&pool, &label, book, chapter, version).await;
    let worker = NotificationWorker::new(
        PgOutbox::new(pool.clone()),
        PgNotificationRecipients::new(pool.clone()),
    );
    assert_eq!(
        worker.run_one().await.unwrap(),
        NotificationRun::Staged {
            recipients: 201,
            pages: 3
        }
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM notification_event_recipient_intents
         WHERE event_id=$1::uuid",
    )
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 201);
    let receipts: Vec<String> = sqlx::query_scalar(
        "SELECT consumer FROM event_deliveries WHERE event_id=$1::uuid ORDER BY consumer",
    )
    .bind(&event_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(receipts, vec!["subscription-inbox-v1"]);
    let final_state: (bool, bool, i32, bool) = sqlx::query_as(
        "SELECT delivered_at IS NULL,
                (SELECT completed_at IS NOT NULL
                 FROM notification_event_fanout_scans
                 WHERE event_id=e.id),
                attempts,lease_until IS NULL
         FROM outbox_events AS e WHERE id=$1::uuid",
    )
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(final_state, (true, true, 1, true));
    // Already-acknowledged subscriber consumer must not reclaim this event.
    let claimed = PgOutbox::new(pool.clone())
        .claim_next_for_consumer("ChapterPublished", "subscription-inbox-v1")
        .await
        .unwrap();
    assert_ne!(claimed.as_ref().map(|e| e.id.as_str()), Some(event_id.as_str()));
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PG18 /tor_ci"]
async fn g_08_worker_dead_letters_malformed_event_but_not_other_consumer() {
    let pool = database().await;
    let nonce: i64 = sqlx::query_scalar(
        "SELECT (EXTRACT(EPOCH FROM clock_timestamp()) * 1000000)::bigint",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let label = format!("bad-{nonce}");
    // The older, unrelated event must never be claimed by the G08 worker.
    let unrelated_id: String = sqlx::query_scalar(
        "INSERT INTO outbox_events
            (event_type,schema_version,aggregate_kind,aggregate_id,
             dedupe_key,payload,required_consumers,next_attempt_at)
         VALUES('PurchaseCommitted',1,'purchase',0,$1,'{}'::jsonb,
                ARRAY['earnings-projection-v1'],now()-interval '20 days')
         RETURNING id::text",
    )
    .bind(format!("notify-unrelated:{label}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let bad_id: String = sqlx::query_scalar(
        "INSERT INTO outbox_events
            (event_type,schema_version,aggregate_kind,aggregate_id,
             dedupe_key,payload,required_consumers,next_attempt_at)
         VALUES('ChapterPublished',1,'chapter',0,$1,'{}'::jsonb,
                ARRAY['subscription-inbox-v1'],now()-interval '10 days')
         RETURNING id::text",
    )
    .bind(format!("notify-invalid:{label}"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let worker = NotificationWorker::new(
        PgOutbox::new(pool.clone()),
        PgNotificationRecipients::new(pool.clone()),
    );
    assert_eq!(worker.run_one().await.unwrap(), NotificationRun::DeadLetter);
    let (invalid_dead, unrelated_dead): (bool, bool) = sqlx::query_as(
        "SELECT
         (SELECT dead_at IS NOT NULL FROM outbox_events WHERE id=$1::uuid),
         (SELECT dead_at IS NOT NULL FROM outbox_events WHERE id=$2::uuid)",
    )
    .bind(&bad_id)
    .bind(&unrelated_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(invalid_dead);
    assert!(!unrelated_dead);
    pool.close().await;
}
