//! RIL-10: real PG18 long-window histograms from RIL-06, read-only/no PII.
use sqlx::postgres::PgPoolOptions;
use std::time::Duration;
use tor_traffic_behavior::{
    repositories::PgRilRollups, BehaviorEvent, LatencyGroup, RollupDelta, RouteCode, StatusGroup,
};
use uuid::Uuid;

fn selected(rows: &[tor_traffic_behavior::domain::RouteCount]) -> i64 {
    rows.iter()
        .filter(|row| {
            row.route == RouteCode::Ready as i16
                && row.status == StatusGroup::ServerError as i16
                && row.latency == LatencyGroup::Slow as i16
        })
        .map(|row| row.count)
        .sum()
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 with official TOR migrations"]
async fn ril_10_rank_input_is_retained_and_read_only_across_repo_restart() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated CI PG URL");
    assert!(url.ends_with("/tor_ci"), "never inspect production DB in CI");
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&url)
        .await
        .expect("PG18 connection");
    let repo = PgRilRollups::new(pool.clone());
    let before = repo.read_cost_buckets(7).await.expect("bounded read");
    assert!(repo.read_cost_buckets(0).await.is_err());
    assert!(repo.read_cost_buckets(31).await.is_err());

    let mut delta = RollupDelta::default();
    for _ in 0..3 {
        delta.record(
            BehaviorEvent {
                route: RouteCode::Ready,
                status: StatusGroup::ServerError,
                latency: LatencyGroup::Slow,
                subject: None,
                at_millis: 1,
            },
            None,
        );
    }
    let id = Uuid::new_v4().to_string();
    assert!(repo.enqueue(&id, &delta).await.unwrap());
    let claim = repo.claim_next().await.unwrap().expect("queued");
    assert_eq!(claim.id, id);
    assert!(repo.materialize(&claim).await.unwrap());
    let resumed = PgRilRollups::new(pool.clone());
    let after = resumed.read_cost_buckets(7).await.unwrap();
    assert_eq!(selected(&after) - selected(&before), 3);
    assert!(after.len() <= 15 * 6 * 4);
    assert!(after.iter().all(|row| {
        (0..=14).contains(&row.route)
            && (0..=5).contains(&row.status)
            && (0..=3).contains(&row.latency)
            && row.count > 0
    }));
    assert!(!resumed.read_cost_buckets(30).await.unwrap().is_empty());
    pool.close().await;
}
