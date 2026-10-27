//! EVT-02/03/04: a validated final free position commits once with outbox.
//! Run only against the disposable PostgreSQL 18 database used by TOR CI.
use sqlx::{PgPool, Row};
use tor_reading::{
    repositories::PgReadingSync,
    services::{
        progress::ReadingPlace,
        sync::SyncError,
    },
};

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL on disposable tor_ci"]
async fn c_13_final_free_progress_is_atomic_durable_and_unique() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "never test against production");
    let pool = PgPool::connect(&url).await.unwrap();

    // Repeatable fixture even after an interrupted previous CI execution.
    sqlx::query("DELETE FROM users WHERE id=98401")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE id=98410")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM outbox_events WHERE dedupe_key='chapter-completed:98401:98420'")
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query(
        "INSERT INTO users(id,email_normalized,display_name,password_hash)
         OVERRIDING SYSTEM VALUE VALUES
         (98401,'c13-reader@example.invalid','C13 Reader','$argon2id$fixture')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO books(id,source_identity,target_language,title,visibility)
         VALUES(98410,'ci:c13:completion','ru','Книга с завершением','public')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapters(id,book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES(98420,98410,1,'Глава','public','free',0)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_versions(id,chapter_id,version_no,body_markdown)
         VALUES(98431,98420,1,'Первая часть\\n\\nКонец😀'),
               (98432,98420,2,'Новая редакция')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_publications(chapter_id,version_id) VALUES(98420,98431)",
    )
    .execute(&pool)
    .await
    .unwrap();

    let sync = PgReadingSync::new(pool.clone());
    let mid = ReadingPlace {
        block_index: 1,
        char_offset: 3,
    };
    assert_eq!(
        sync.save_free(98401, 98420, 98431, mid, 0)
            .await
            .unwrap()
            .revision,
        1
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM reader_chapter_completions WHERE user_id=98401",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0, "middle of chapter is not completed");

    let final_place = ReadingPlace {
        block_index: 2,
        char_offset: 7, // Конец😀 is seven UTF-16 units.
    };
    let a = sync.clone();
    let b = sync.clone();
    let (left, right) = tokio::join!(
        a.save_free(98401, 98420, 98431, final_place, 1),
        b.save_free(98401, 98420, 98431, final_place, 1)
    );
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert!(left == Err(SyncError::Conflict) || right == Err(SyncError::Conflict));

    let stored = PgReadingSync::new(pool.clone())
        .free_state(98401, 98420)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.receipt.revision, 2);
    assert_eq!(stored.place, final_place);

    let completion = sqlx::query(
        "SELECT book_id,chapter_id,first_version_id,source_event_id::text AS event_id
         FROM reader_chapter_completions WHERE user_id=98401",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(completion.get::<i64, _>("book_id"), 98410);
    assert_eq!(completion.get::<i64, _>("chapter_id"), 98420);
    assert_eq!(completion.get::<i64, _>("first_version_id"), 98431);
    let event_id: String = completion.get("event_id");
    let published: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox_events
         WHERE id=$1::uuid AND event_type='ChapterCompletedV1'
           AND required_consumers=ARRAY['ranking-eligibility-v1']",
    )
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(published, 1, "completion and event share one transaction");

    // Re-publication and saving an entirely new chapter edition must not
    // grant another completion for the same reader/chapter.
    sqlx::query("UPDATE chapter_publications SET version_id=98432 WHERE chapter_id=98420")
        .execute(&pool)
        .await
        .unwrap();
    let new_final = ReadingPlace {
        block_index: 1,
        char_offset: "Новая редакция".encode_utf16().count() as i32,
    };
    assert_eq!(
        sync.save_free(98401, 98420, 98432, new_final, 2)
            .await
            .unwrap()
            .revision,
        3
    );
    assert_eq!(
        sync.save_free(98401, 98420, 98431, final_place, 3)
            .await,
        Err(SyncError::Conflict),
        "a late V1 device cannot replay a completion"
    );
    let num_facts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM reader_chapter_completions WHERE user_id=98401",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let num_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox_events
         WHERE dedupe_key='chapter-completed:98401:98420'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((num_facts, num_events), (1, 1));

    sqlx::query("UPDATE chapters SET admin_hold=true WHERE id=98420")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        sync.save_free(98401, 98420, 98432, new_final, 3)
            .await,
        Err(SyncError::Conflict),
        "moderation blocks verified progress"
    );

    sqlx::query("DELETE FROM users WHERE id=98401")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM books WHERE id=98410")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM outbox_events WHERE id=$1::uuid")
        .bind(&event_id)
        .execute(&pool)
        .await
        .unwrap();
}
