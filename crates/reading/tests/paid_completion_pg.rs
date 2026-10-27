//! C-09/F-05: an authorized purchased pinned V1 produces one durable C fact.
//! Only CI disposable PostgreSQL is allowed; S3 GET and grant are not completion.
use sqlx::{PgPool, Row};
use tor_reading::{
    repositories::PgReadingSync,
    services::{progress::ReadingPlace, sync::SyncError},
};

#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending in tor_ci"]
async fn c_14_paid_v1_completion_survives_v2_and_refund_blocks_new_fact() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.ends_with("/tor_ci"), "never run against production");
    let pool = PgPool::connect(&url).await.unwrap();
    // D purchase, library and revocation facts are append-only (no DELETE).
    // The entire tor_ci database is disposable; fixed fixtures require a fresh
    // CI schema. Do not fake cleanup by mutating D settlement history.
    sqlx::query(
        "INSERT INTO users(id,email_normalized,display_name,password_hash)
         OVERRIDING SYSTEM VALUE VALUES
         (98601,'c14-paid-a@example.invalid','Paid A','$argon2id$fixture'),
         (98602,'c14-paid-b@example.invalid','Paid B','$argon2id$fixture')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO books(id,source_identity,target_language,title,visibility)
         VALUES(98610,'ci:c14:paid','ru','Платная V1','public')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapters(id,book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES(98620,98610,1,'Платная глава','public','paid',100)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_versions(id,chapter_id,version_no,body_markdown)
         VALUES (98631,98620,1,E'А😀Б\\n\\nКонец😀'),
                (98632,98620,2,'Новая редакция')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES(98620,98631)")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_purchase_facts
           (id,user_id,chapter_id,version_id,settlement_reference,amount_minor,currency)
         VALUES
           (98640,98601,98620,98631,'ci-c14-paid-a',100,'RUB'),
           (98641,98602,98620,98631,'ci-c14-paid-b',100,'RUB')",
    )
    .execute(&pool)
    .await
    .unwrap();

    let repo = PgReadingSync::new(pool.clone());
    let early = ReadingPlace {
        block_index: 1,
        char_offset: 2,
    };
    assert_eq!(
        repo.save_purchased(98601, 98640, 98620, 98631, early, 0)
            .await
            .unwrap()
            .revision,
        1
    );
    let middle_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reader_chapter_completions WHERE user_id=98601")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(middle_count, 0, "intermediate cursor is not a read fact");
    assert_eq!(
        repo.save_purchased(
            98601,
            98640,
            98620,
            98631,
            ReadingPlace {
                block_index: 2,
                char_offset: 99,
            },
            1,
        )
        .await,
        Err(SyncError::Invalid),
        "the repository must recheck the immutable paid text, not trust HTTP"
    );
    let last = ReadingPlace {
        block_index: 2,
        char_offset: 7, // Конец😀: seven UTF-16 units
    };
    let left = repo.clone();
    let right = repo.clone();
    let (one, two) = tokio::join!(
        left.save_purchased(98601, 98640, 98620, 98631, last, 1),
        right.save_purchased(98601, 98640, 98620, 98631, last, 1)
    );
    assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
    assert!(one == Err(SyncError::Conflict) || two == Err(SyncError::Conflict));
    let c = sqlx::query(
        "SELECT book_id,first_version_id,purchase_id,source_event_id::text AS id
         FROM reader_chapter_completions WHERE user_id=98601 AND chapter_id=98620",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(c.get::<i64, _>("book_id"), 98610);
    assert_eq!(c.get::<i64, _>("first_version_id"), 98631);
    assert_eq!(c.get::<Option<i64>, _>("purchase_id"), Some(98640));
    let event_id: String = c.get("id");
    let envelope: String = sqlx::query_scalar(
        "SELECT payload::text FROM outbox_events
         WHERE id=$1::uuid AND event_type='ChapterCompletedV1'
           AND aggregate_kind='chapter'
           AND required_consumers=ARRAY['ranking-eligibility-v1']",
    )
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(envelope.contains("C-verified-purchased-position"));
    assert!(envelope.contains("98640"));
    assert!(!envelope.contains("Конец"));
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox_events WHERE dedupe_key='chapter-completed:98601:98620'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);

    // A new public V2 cannot replace the purchased V1 in the completion.
    sqlx::query("UPDATE chapter_publications SET version_id=98632 WHERE chapter_id=98620")
        .execute(&pool)
        .await
        .unwrap();
    let reopened = repo
        .purchased_state(98601, 98640, 98620, 98631)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((reopened.version_id, reopened.receipt.revision), (98631, 2));
    assert_eq!(
        repo.save_purchased(98601, 98640, 98620, 98631, last, 2)
            .await,
        Err(SyncError::Conflict)
    );
    assert_eq!(
        repo.save_purchased(98602, 98640, 98620, 98631, last, 0)
            .await,
        Err(SyncError::NotFound),
        "another user's purchase never authorizes completion"
    );

    // D revocation stops a NEW completion even when the consumer still
    // retains historical facts from an earlier legitimately finished chapter.
    sqlx::query(
        "INSERT INTO chapter_purchase_revocations(purchase_id,reversal_reference,reason)
         VALUES (98641,'ci-c14-refund-b','refund'),(98640,'ci-c14-refund-a','refund')",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        repo.save_purchased(98602, 98641, 98620, 98631, last, 0)
            .await,
        Err(SyncError::NotFound)
    );
    assert_eq!(
        repo.save_purchased(98601, 98640, 98620, 98631, last, 2)
            .await,
        Err(SyncError::NotFound)
    );
    let number_of_completions: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM reader_chapter_completions WHERE chapter_id=98620",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(number_of_completions, 1);

    // Only C completion and outbox are disposable; purchase history is not.
    sqlx::query("DELETE FROM reader_chapter_completions WHERE user_id=98601 AND chapter_id=98620")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM outbox_events WHERE id=$1::uuid")
        .bind(&event_id)
        .execute(&pool)
        .await
        .unwrap();
}
