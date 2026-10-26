//! Real PostgreSQL 18 evidence: stage, claim/fence, crash recovery and retention.
use sqlx::{postgres::PgPoolOptions, Row};
use std::time::Duration;
use tor_traffic_behavior::{
    domain::{RouteCount, RollupDelta},
    repositories::PgRilRollups,
    services::PgRollupPump,
    BehaviorEvent, GapBucket, LatencyGroup, RouteCode, StatusGroup, TrafficRecorder, Transition,
};
use uuid::Uuid;

fn sample() -> RollupDelta {
    let mut delta = RollupDelta::default();
    let make = |route, status, latency| BehaviorEvent {
        route, status, latency, subject: None, at_millis: 1,
    };
    delta.record(make(RouteCode::Books, StatusGroup::Success, LatencyGroup::Under100Ms), None);
    delta.record(make(RouteCode::Books, StatusGroup::Success, LatencyGroup::Under100Ms), None);
    delta.record(
        make(RouteCode::Books, StatusGroup::ServerError, LatencyGroup::Slow),
        Some(Transition {
            from: RouteCode::Catalog, to: RouteCode::Books, gap: GapBucket::Fast,
        }),
    );
    delta
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 with official TOR migrations"]
async fn ril_06_pg_crash_replay_fencing_and_daily_rollups() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated CI PG URL");
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&url).await.unwrap();
    let repo = PgRilRollups::new(pool.clone());
    let id = Uuid::new_v4().to_string();
    let batch = sample();
    assert_eq!(batch.route_rows().iter().map(|v| v.count).sum::<i64>(), 3);
    assert_eq!(batch.transition_rows().len(), 1);

    let before: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(request_count),0)::bigint FROM ril_route_hourly
          WHERE bucket=date_trunc('hour',now()) AND route_code=$1",
    ).bind(RouteCode::Books as i16).fetch_one(&pool).await.unwrap();

    assert!(repo.enqueue(&id, &batch).await.unwrap());
    assert!(!repo.enqueue(&id, &batch).await.unwrap(), "same batch must dedupe");
    let (one, two) = tokio::join!(repo.claim_next(), repo.claim_next());
    let claims: Vec<_> = [one.unwrap(), two.unwrap()].into_iter().flatten().collect();
    assert_eq!(claims.len(), 1, "SKIP LOCKED prohibits concurrent claims");
    let first = &claims[0];
    assert_eq!(first.id, id);
    assert_eq!(first.epoch, 1);

    // Simulate a process crashing after claim but before committing counters.
    sqlx::query(
        "UPDATE ril_flush_batches
            SET leased_until=now()-INTERVAL '1 second'
          WHERE batch_id=$1",
    ).bind(&id).execute(&pool).await.unwrap();
    let reloaded = PgRilRollups::new(pool.clone()); // a new process
    let second = reloaded.claim_next().await.unwrap().expect("expired claim");
    assert_eq!(second.epoch, 2);
    assert!(!repo.materialize(first).await.unwrap(), "old epoch fenced");
    assert!(reloaded.materialize(&second).await.unwrap());
    assert!(!repo.materialize(&second).await.unwrap(), "done is never re-applied");
    assert_eq!(reloaded.claim_next().await.unwrap().map(|c| c.id), None);

    let after: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(request_count),0)::bigint FROM ril_route_hourly
          WHERE bucket=date_trunc('hour',now()) AND route_code=$1",
    ).bind(RouteCode::Books as i16).fetch_one(&pool).await.unwrap();
    assert_eq!(after - before, 3);

    let row = sqlx::query(
        "SELECT requests,http_errors,slow_requests FROM ril_daily_rollup
         WHERE day=current_date AND route_code=$1",
    ).bind(RouteCode::Books as i16).fetch_one(&pool).await.unwrap();
    assert!(row.try_get::<i64, _>("requests").unwrap() >= 3);
    assert!(row.try_get::<i64, _>("http_errors").unwrap() >= 1);
    assert!(row.try_get::<i64, _>("slow_requests").unwrap() >= 1);
    let transitions: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(transition_count),0)::bigint
           FROM ril_transition_hourly
          WHERE bucket=date_trunc('hour',now())
            AND from_code=$1 AND to_code=$2 AND gap_group=$3",
    ).bind(RouteCode::Catalog as i16)
     .bind(RouteCode::Books as i16)
     .bind(GapBucket::Fast as i16)
     .fetch_one(&pool).await.unwrap();
    assert!(transitions >= 1);

    // A forced failed attempt returns to the queue without duplicate application.
    let retry_id = Uuid::new_v4().to_string();
    assert!(repo.enqueue(&retry_id, &batch).await.unwrap());
    let claim = repo.claim_next().await.unwrap().unwrap();
    assert_eq!(claim.id, retry_id);
    assert!(repo.retry(&claim).await.unwrap());
    assert!(!repo.materialize(&claim).await.unwrap());
    sqlx::query("UPDATE ril_flush_batches SET run_after=now()-INTERVAL '1 second'
                 WHERE batch_id=$1")
        .bind(&retry_id).execute(&pool).await.unwrap();
    let recovered = repo.claim_next().await.unwrap().unwrap();
    assert_eq!(recovered.epoch, 2);
    assert!(repo.materialize(&recovered).await.unwrap());
    assert!(!repo.materialize(&recovered).await.unwrap());
    let final_count: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(request_count),0)::bigint FROM ril_route_hourly
          WHERE bucket=date_trunc('hour',now()) AND route_code=$1",
    ).bind(RouteCode::Books as i16).fetch_one(&pool).await.unwrap();
    assert_eq!(final_count - before, 6);

    // Retention removes only finished old batches. Pending data remains
    // recoverable; old summaries and their tombstones are independently scoped.
    sqlx::query(
        "UPDATE ril_flush_batches SET finished_at=now()-INTERVAL '91 days'
         WHERE batch_id=$1",
    ).bind(&id).execute(&pool).await.unwrap();
    repo.prune_expired().await.unwrap();
    let old: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM ril_flush_batches WHERE batch_id=$1",
    ).bind(&id).fetch_one(&pool).await.unwrap();
    assert_eq!(old, 0);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 with official TOR migrations"]
async fn ril_06_background_sink_survives_pg_loss_without_failing_http() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated CI PG URL");
    let pool = PgPoolOptions::new().max_connections(2)
        .connect(&url).await.unwrap();
    let repo = PgRilRollups::new(pool.clone());
    let recorder = TrafficRecorder::start(32, 32);
    assert!(recorder.try_record(BehaviorEvent {
        route: RouteCode::Catalog,
        status: StatusGroup::Success,
        latency: LatencyGroup::Under10Ms,
        subject: None, at_millis: 200,
    }));
    let mut pump = PgRollupPump::new(repo, recorder.clone());
    pump.run_once().await;
    let total: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(request_count),0)::bigint
           FROM ril_route_hourly WHERE route_code=$1
            AND bucket=date_trunc('hour',now())",
    ).bind(RouteCode::Catalog as i16).fetch_one(&pool).await.unwrap();
    assert!(total >= 1, "a true batch was stored in PG");
    pool.close().await;
    assert!(recorder.try_record(BehaviorEvent {
        route: RouteCode::Catalog,
        status: StatusGroup::Success,
        latency: LatencyGroup::Under10Ms,
        subject: None, at_millis: 500,
    }), "primary request remains independent of PG");
    pump.run_once().await; // best effort: no panic on failed PG
    assert_eq!(recorder.dropped_events(), 0);
}
