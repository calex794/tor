//! Current-schema business proof with the old optional collection switches on.
//! The database barrier must never make canonical reading depend on analytics.
use sqlx::{
    postgres::{PgConnectOptions, PgPoolOptions},
    PgPool,
};
use std::str::FromStr;
use tor_reading::{
    repositories::{
        flow_capture::PgFlowCapture, sync::PurchasedFinish, PgBookFollows, PgLibrary, PgReader,
        PgReadingProgress, PgReadingSync,
    },
    services::{progress::ReadingPlace, sync::SyncError, PublicReadingRepository},
};

#[path = "collection_retirement_pg/fixtures.rs"]
mod fixtures;
#[path = "collection_retirement_pg/paid.rs"]
mod paid;
use fixtures::*;

#[tokio::test]
#[ignore = "requires current disposable PostgreSQL tor_ci at migration 192"]
async fn retired_capture_preserves_exact_library_follow_pins_and_transactions() {
    let pool = pool(4).await;
    let reader = actor(&pool, "actions").await;
    let one = book(&pool, false).await;
    let baseline = optional_counts(&pool).await;
    let monitor = PgFlowCapture::new(pool.clone());
    let health = monitor.health();
    let library = PgLibrary::new(pool.clone()).with_flow_collection(true);
    let follows = PgBookFollows::new(pool.clone()).with_flow_collection(true);
    let (first, second) = tokio::join!(
        library.add_book(reader, one.id),
        library.add_book(reader, one.id)
    );
    first.unwrap();
    second.unwrap();
    let (first, second) = tokio::join!(
        follows.follow(reader, one.id),
        follows.follow(reader, one.id)
    );
    first.unwrap();
    second.unwrap();
    assert_eq!(
        business_actions(&pool, reader, one.id).await,
        (1, 1, one.version)
    );
    let accepted: (String, String) = sqlx::query_as(
        "SELECT l.created_at::text,f.created_at::text FROM reader_library_items l
         JOIN reader_book_follows f USING(user_id,book_id) WHERE l.user_id=$1 AND l.book_id=$2",
    )
    .bind(reader)
    .bind(one.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let second_version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'new edition') RETURNING id",
    )
    .bind(one.chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(one.chapter)
        .bind(second_version)
        .execute(&pool)
        .await
        .unwrap();
    library.add_book(reader, one.id).await.unwrap();
    follows.follow(reader, one.id).await.unwrap();
    assert_eq!(
        business_actions(&pool, reader, one.id).await,
        (1, 1, one.version)
    );
    let replayed: (String, String) = sqlx::query_as(
        "SELECT l.created_at::text,f.created_at::text FROM reader_library_items l
         JOIN reader_book_follows f USING(user_id,book_id) WHERE l.user_id=$1 AND l.book_id=$2",
    )
    .bind(reader)
    .bind(one.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        accepted, replayed,
        "replay must preserve accepted action timestamps"
    );
    library.remove_book(reader, one.id).await.unwrap();
    follows.unfollow(reader, one.id).await.unwrap();
    library.add_book(reader, one.id).await.unwrap();
    follows.follow(reader, one.id).await.unwrap();
    assert_eq!(
        business_actions(&pool, reader, one.id).await,
        (1, 1, second_version)
    );

    let rollback = book(&pool, false).await;
    let mut transaction = pool.begin().await.unwrap();
    sqlx::query("SELECT set_config('tor.flow_capture_enabled','1',true)")
        .execute(&mut *transaction)
        .await
        .unwrap();
    sqlx::query("INSERT INTO reader_book_follows(user_id,book_id) VALUES($1,$2)")
        .bind(reader)
        .bind(rollback.id)
        .execute(&mut *transaction)
        .await
        .unwrap();
    transaction.rollback().await.unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM reader_book_follows WHERE user_id=$1 AND book_id=$2"
        )
        .bind(reader)
        .bind(rollback.id)
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    sqlx::query("UPDATE books SET admin_hold=true WHERE id=$1")
        .bind(rollback.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(library.add_book(reader, rollback.id).await.is_err());
    assert!(follows.follow(reader, rollback.id).await.is_err());
    assert_eq!(
        monitor.health(),
        health,
        "retired action callback is an inert no-op"
    );
    assert_no_optional(&pool, reader, &baseline).await;
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires current disposable PostgreSQL tor_ci at migration 192"]
async fn old_capture_switches_keep_free_and_manual_progress_canonical_and_replay_safe() {
    let pool = pool(1).await;
    let reader = actor(&pool, "free").await;
    let one = book(&pool, false).await;
    let manual_book = book(&pool, false).await;
    let baseline = optional_counts(&pool).await;
    let sync = legacy_sync(&pool);
    let manual = PgReadingProgress::new(pool.clone())
        .with_activity_collection(true)
        .with_book_return_collection(true)
        .with_flow_collection(true);
    assert!(PgReader::new(pool.clone())
        .public_chapter(one.chapter as u64)
        .await
        .unwrap()
        .is_some());
    assert!(sync
        .free_state(reader, one.chapter)
        .await
        .unwrap()
        .is_none());
    assert_eq!(canonical_counts(&pool, reader).await, (0, 0));
    manual
        .save_position(reader, manual_book.chapter, manual_book.version, place(2))
        .await
        .unwrap();
    assert_eq!(
        manual
            .saved_place(reader, manual_book.chapter, manual_book.version)
            .await
            .unwrap(),
        Some(place(2))
    );
    assert_eq!(
        canonical_counts(&pool, reader).await,
        (0, 0),
        "native saved position is a business cursor, not a canonical start"
    );
    assert_eq!(
        sync.save_free(reader, one.chapter, one.version, place(2), 0)
            .await
            .unwrap()
            .revision,
        1
    );
    assert_eq!(
        sync.save_free(reader, one.chapter, one.version, place(10), 1)
            .await
            .unwrap()
            .revision,
        2
    );
    assert_eq!(canonical_counts(&pool, reader).await, (1, 1));
    assert_canonical(&pool, reader, one, None).await;
    let saved = canonical_snapshot(&pool, reader).await;
    assert_eq!(
        sync.save_free(reader, one.chapter, one.version, place(10), 2)
            .await,
        Err(SyncError::Conflict)
    );
    assert_eq!(
        sync.save_free(reader, one.chapter, one.version, place(99), 2)
            .await,
        Err(SyncError::Invalid)
    );
    assert_eq!(
        canonical_snapshot(&pool, reader).await,
        saved,
        "failed/replayed commands preserve source event, payload, version and business consumers"
    );
    let current = sync.free_state(reader, one.chapter).await.unwrap().unwrap();
    assert_eq!(
        (
            current.chapter_id,
            current.version_id,
            current.place,
            current.receipt.revision
        ),
        (one.chapter, one.version, place(10), 2)
    );
    let gates: (bool, bool) = sqlx::query_as(
        "SELECT COALESCE(current_setting('tor.flow_capture_enabled',true),'')='1',
          COALESCE(current_setting('tor.flow_read_capture_enabled',true),'')='1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        gates,
        (false, false),
        "SET LOCAL cannot leak through the one-connection pool"
    );
    assert_no_optional(&pool, reader, &baseline).await;

    // Current factories remain default-off and must not report spurious
    // optional failures. Old explicit true writers may hit the statement fence
    // inside their savepoint; that truthful diagnostic cannot fail C progress.
    let ordinary = book(&pool, false).await;
    let monitor = PgFlowCapture::new(pool.clone());
    let health = monitor.health();
    PgReadingSync::new(pool.clone())
        .save_free(reader, ordinary.chapter, ordinary.version, place(2), 0)
        .await
        .unwrap();
    assert_eq!(monitor.health(), health);
    assert_no_optional(&pool, reader, &baseline).await;
    pool.close().await;
}
