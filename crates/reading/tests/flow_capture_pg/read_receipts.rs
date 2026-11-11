use super::*;

#[tokio::test]
#[ignore = "requires explicit disposable tor_ci_legacy_analytics at migration 191"]
async fn per_command_read_proof_cannot_backfill_disabled_canonical_events() {
    let pool = pool(2).await;
    let reader = actor(&pool, "command-gate").await;
    let (subject, _) = consent(&pool, reader, true).await;
    let on_book = book(&pool, false).await;
    let off_book = book(&pool, false).await;
    let enabled = PgReadingSync::new(pool.clone()).with_flow_collection(true);
    let disabled = PgReadingSync::new(pool.clone());
    let monitor = PgFlowCapture::new(pool.clone());
    let failures = monitor.health().read_failures;
    enabled
        .save_free(reader, on_book.chapter, on_book.version, place(2), 0)
        .await
        .unwrap();
    disabled
        .save_free(reader, off_book.chapter, off_book.version, place(2), 0)
        .await
        .unwrap();
    assert_eq!(reads(&pool, reader, on_book.id).await, vec![0]);
    assert!(reads(&pool, reader, off_book.id).await.is_empty());
    let canonical: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reader_chapter_starts WHERE user_id=$1")
            .bind(reader)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        canonical, 2,
        "operational C history is independent from optional FLOW"
    );
    enabled
        .save_free(reader, off_book.chapter, off_book.version, place(3), 1)
        .await
        .unwrap();
    assert!(
        reads(&pool, reader, off_book.id).await.is_empty(),
        "replaying the old canonical start cannot manufacture missing proof"
    );
    enabled
        .save_free(reader, off_book.chapter, off_book.version, place(10), 2)
        .await
        .unwrap();
    assert_eq!(
        reads(&pool, reader, off_book.id).await,
        vec![1],
        "only this newly accepted completion has FLOW permission"
    );
    assert_eq!(
        monitor.health().read_failures,
        failures,
        "off and replay are not failures"
    );
    let inherited: bool = sqlx::query_scalar(
        "SELECT COALESCE(current_setting('tor.flow_read_capture_enabled',true),'')='1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(!inherited);
    consent(&pool, reader, false).await;
    let retained: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM reader_flow_read_receipts WHERE subject_id=$1::uuid",
    )
    .bind(subject)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        retained, 0,
        "withdrawal erases read permission receipts atomically"
    );
    erase_scopes(&pool, &[reader]).await;
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires explicit disposable tor_ci_legacy_analytics at migration 191"]
async fn optional_read_receipt_failure_preserves_canonical_progress_and_outbox() {
    let pool = pool(2).await;
    let reader = actor(&pool, "read-outage").await;
    consent(&pool, reader, true).await;
    let one = book(&pool, false).await;
    let enabled = PgReadingSync::new(pool.clone()).with_flow_collection(true);
    let monitor = PgFlowCapture::new(pool.clone());
    let before = monitor.health().read_failures;
    let mut lock = pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE reader_flow_read_receipts IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let saved = tokio::time::timeout(
        std::time::Duration::from_secs(1),
        enabled.save_free(reader, one.chapter, one.version, place(2), 0),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(saved.revision, 1);
    assert_eq!(monitor.health().read_failures, before + 1);
    lock.rollback().await.unwrap();
    assert!(reads(&pool, reader, one.id).await.is_empty());
    let canonical: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM reader_chapter_starts s JOIN outbox_events e ON e.id=s.source_event_id
         WHERE s.user_id=$1 AND s.book_id=$2 AND e.event_type='ChapterStartedV1'",
    ).bind(reader).bind(one.id).fetch_one(&pool).await.unwrap();
    assert_eq!(
        canonical, 1,
        "canonical C fact and its real outbox survive optional failure"
    );
    assert_eq!(
        days(&pool, reader).await,
        1,
        "independent positive day capture can succeed"
    );
    enabled
        .save_free(reader, one.chapter, one.version, place(3), 1)
        .await
        .unwrap();
    assert!(reads(&pool, reader, one.id).await.is_empty());
    enabled
        .save_free(reader, one.chapter, one.version, place(10), 2)
        .await
        .unwrap();
    assert_eq!(reads(&pool, reader, one.id).await, vec![1]);
    erase_scopes(&pool, &[reader]).await;
    pool.close().await;
}
