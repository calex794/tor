use sqlx::postgres::PgPoolOptions;
use std::time::Duration;
use tor_recommendations::{
    domain::feedback::FeedbackError, repositories::PgRecommendations,
};

#[tokio::test]
#[ignore = "requires the installed retirement migration on disposable tor_ci"]
async fn retirement_adapter_bounds_transactions_and_does_not_wait_for_a_busy_gate() {
    let url = super::current_analytics::url("TEST_DATABASE_URL", "tor_app");
    let pool = PgPoolOptions::new().max_connections(1).connect(&url).await.unwrap();
    super::current_analytics::assert_schema(&pool).await;
    let locker = PgPoolOptions::new().max_connections(1).connect(&url).await.unwrap();
    let store = PgRecommendations::new(pool.clone());
    for limit in [0, 1001] {
        assert_eq!(store.retire_analytics_batch(limit).await, Err(FeedbackError::Invalid));
    }
    let settings: (String, String) = sqlx::query_as(
        "SELECT current_setting('lock_timeout'),current_setting('statement_timeout')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let mut held_gate = locker.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(20261116,8903)")
        .execute(&mut *held_gate)
        .await
        .unwrap();
    let busy = tokio::time::timeout(Duration::from_secs(2), store.retire_analytics_batch(256))
        .await
        .expect("a busy retirement gate must not wait")
        .unwrap();
    assert_eq!(busy, (0, true), "busy is pending work, never a drained acknowledgement");
    held_gate.rollback().await.unwrap();

    // Call the actual SQL function at both limits and the production size.
    // The I installed-upgrade suite owns data-filled erasure and preservation.
    for limit in [1, 256, 1000] {
        let (erased_rows, _) = store.retire_analytics_batch(limit).await.unwrap();
        assert!(erased_rows <= limit as u64);
    }
    let mut held_table = locker.begin().await.unwrap();
    sqlx::query("LOCK TABLE rec_dirty_buckets IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *held_table)
        .await
        .unwrap();
    let timed_out = tokio::time::timeout(Duration::from_secs(2), store.retire_analytics_batch(256))
        .await
        .expect("the wrapper sets timeouts before entering the SQL function");
    assert_eq!(timed_out, Err(FeedbackError::Unavailable));
    held_table.rollback().await.unwrap();
    let after: (String, String) = sqlx::query_as(
        "SELECT current_setting('lock_timeout'),current_setting('statement_timeout')",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after, settings, "transaction-local bounds cannot leak into the pool");
    store.retire_analytics_batch(1).await.unwrap();
    locker.close().await;
    pool.close().await;
}
