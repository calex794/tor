use super::*;
use std::time::{Duration, Instant};

#[tokio::test]
#[ignore = "requires explicit disposable tor_ci_legacy_analytics at migration 191"]
async fn capture_lock_failures_preserve_business_and_report_process_failures() {
    let pool = pool(2).await;
    let reader = actor(&pool, "lock-outage").await;
    consent(&pool, reader, true).await;
    let one = book(&pool, false).await;
    let library = PgLibrary::new(pool.clone()).with_flow_collection(true);
    let follows = PgBookFollows::new(pool.clone()).with_flow_collection(true);
    let health = PgFlowCapture::new(pool.clone());
    let before = health.health();

    let mut blocked = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE reader_flow_book_actions IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocked)
        .await
        .unwrap();
    let started = Instant::now();
    tokio::time::timeout(Duration::from_secs(1), library.add_book(reader, one.id))
        .await
        .expect("optional capture cannot hold the business command")
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), follows.follow(reader, one.id))
        .await
        .expect("optional capture cannot hold the business command")
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(1));
    assert_eq!(health.health().action_failures, before.action_failures + 2);
    let timeout: String = sqlx::query_scalar("SHOW lock_timeout")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        timeout, "0",
        "function-scoped timeout cannot leak into business SQL"
    );
    blocked.rollback().await.unwrap();
    assert_eq!(actions(&pool, reader, 1).await, 0);
    assert_eq!(actions(&pool, reader, 2).await, 0);
    assert!(follows.is_following(reader, one.id).await.unwrap());
    assert_eq!(
        library
            .list_books(reader, 20, None)
            .await
            .unwrap()
            .items
            .len(),
        1
    );
    let pins: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reader_saved_free_chapters WHERE user_id=$1")
            .bind(reader)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(pins, 1, "analytics failure cannot discard free pins");
    // Replaying the already accepted business state cannot fabricate a missing
    // historical event once optional storage recovers.
    library.add_book(reader, one.id).await.unwrap();
    follows.follow(reader, one.id).await.unwrap();
    assert_eq!(actions(&pool, reader, 1).await, 0);
    assert_eq!(actions(&pool, reader, 2).await, 0);

    let sync = PgReadingSync::new(pool.clone()).with_flow_collection(true);
    let mut blocked = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE reader_flow_activity_days IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocked)
        .await
        .unwrap();
    let saved = tokio::time::timeout(
        Duration::from_secs(1),
        sync.save_free(reader, one.chapter, one.version, place(1), 0),
    )
    .await
    .expect("the savepoint must isolate optional capture contention")
    .unwrap();
    assert_eq!(saved.revision, 1);
    assert_eq!(
        health.health().activity_failures,
        before.activity_failures + 1
    );
    let timeout: String = sqlx::query_scalar("SHOW lock_timeout")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(timeout, "0", "rollback restores the previous timeout");
    blocked.rollback().await.unwrap();
    assert_eq!(days(&pool, reader).await, 0);
    sync.save_free(reader, one.chapter, one.version, place(2), 1)
        .await
        .unwrap();
    assert_eq!(
        days(&pool, reader).await,
        1,
        "new accepted progress recovers capture"
    );

    // An in-flight scope withdrawal holds the same row as capture. A short
    // failed optional observation cannot cross into the revoked epoch later.
    let two = book(&pool, false).await;
    let mut revoke = pool.begin().await.unwrap();
    sqlx::query("UPDATE rec_subjects SET flow_epoch=NULL,flow_enabled_at=NULL WHERE user_id=$1")
        .bind(reader)
        .execute(&mut *revoke)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(1), library.add_book(reader, two.id))
        .await
        .unwrap()
        .unwrap();
    revoke.commit().await.unwrap();
    assert_eq!(actions(&pool, reader, 1).await, 0);
    assert_eq!(days(&pool, reader).await, 0);
    consent(&pool, reader, true).await;
    library.add_book(reader, two.id).await.unwrap();
    assert_eq!(
        actions(&pool, reader, 1).await,
        0,
        "the prior accepted insert is not replayed into a new scope"
    );
    erase_scopes(&pool, &[reader]).await;
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable tor_ci_legacy_analytics at migration 191"]
async fn retention_erases_raw_after_thirty_days_even_without_collection() {
    let pool = pool(2).await;
    let old = actor(&pool, "expired").await;
    let fresh = actor(&pool, "retained").await;
    let (old_subject, _) = consent(&pool, old, true).await;
    consent(&pool, fresh, true).await;
    let one = book(&pool, false).await;
    let library = PgLibrary::new(pool.clone()).with_flow_collection(true);
    let sync = PgReadingSync::new(pool.clone()).with_flow_collection(true);
    for reader in [old, fresh] {
        library.add_book(reader, one.id).await.unwrap();
        sync.save_free(reader, one.chapter, one.version, place(1), 0)
            .await
            .unwrap();
    }
    sqlx::query("UPDATE reader_flow_book_actions SET occurred_at=now()-interval '31 days' WHERE subject_id=$1::uuid")
        .bind(&old_subject).execute(&pool).await.unwrap();
    sqlx::query("UPDATE reader_flow_read_receipts SET occurred_at=now()-interval '31 days' WHERE subject_id=$1::uuid")
        .bind(&old_subject).execute(&pool).await.unwrap();
    sqlx::query(
        "UPDATE reader_flow_activity_days SET first_seen_at=now()-interval '31 days',
           activity_day=((now()-interval '31 days') AT TIME ZONE 'UTC')::date
         WHERE subject_id=$1::uuid",
    )
    .bind(&old_subject)
    .execute(&pool)
    .await
    .unwrap();
    // Maintenance has no collection flag and must work on a public/auth-off start.
    let maintenance = PgFlowCapture::new(pool.clone());
    maintenance.maintain().await.unwrap();
    assert_eq!(actions(&pool, old, 1).await, 0);
    assert_eq!(days(&pool, old).await, 0);
    assert!(reads(&pool, old, one.id).await.is_empty());
    assert_eq!(actions(&pool, fresh, 1).await, 1);
    assert_eq!(days(&pool, fresh).await, 1);
    assert_eq!(reads(&pool, fresh, one.id).await, vec![0]);
    assert_eq!(
        PgLibrary::new(pool.clone())
            .list_books(old, 20, None)
            .await
            .unwrap()
            .items
            .len(),
        1,
        "privacy cleanup must not remove the user's saved book"
    );
    maintenance.maintain().await.unwrap();
    assert_eq!(actions(&pool, fresh, 1).await, 1);
    erase_scopes(&pool, &[old, fresh]).await;
    pool.close().await;
}
