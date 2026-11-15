//! Actual PG projection, immutable retry UUID, skew, retention and local hours.
use sqlx::{postgres::PgPoolOptions, PgPool, Row};
use std::time::Duration;
use tor_contracts::flow::{AcquisitionChannel, RouteSurface};
use tor_traffic_behavior::{
    domain::RollupDelta,
    repositories::{PgFlowAudience, PgRilRollups},
    services::audience::FlowHourlyError,
    FlowPageview,
};
use uuid::Uuid;

pub(super) struct Fixture {
    pub(super) owner: PgPool,
    pub(super) app: PgPool,
    pub(super) schema: String,
}

impl Fixture {
    pub(super) async fn new() -> Self {
        let owner_url = std::env::var("MIGRATOR_URL").unwrap();
        let app_url = std::env::var("TEST_DATABASE_URL").unwrap();
        for url in [&owner_url, &app_url] {
            assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
        }
        let owner = PgPool::connect(&owner_url).await.unwrap();
        let schema = format!("flow_observed_{}", Uuid::new_v4().simple());
        sqlx::query(&format!("CREATE SCHEMA {schema}"))
            .execute(&owner)
            .await
            .unwrap();
        // Exact officially migrated tables, isolated from other acceptance fixtures.
        for table in [
            "ril_flush_batches",
            "ril_flush_routes",
            "ril_flush_transitions",
            "ril_flush_sql_costs",
            "ril_flush_flow",
            "ril_route_hourly",
            "ril_transition_hourly",
            "ril_sql_cost_hourly",
            "ril_daily_rollup",
            "ril_flow_hourly",
            "ril_flush_flow_observed",
            "ril_flow_observed_hourly",
            "ril_flow_observed_receipts",
            "ril_flow_health_epochs",
            "ril_flow_health_hourly",
        ] {
            sqlx::query(&format!(
                "CREATE TABLE {schema}.{table} (LIKE public.{table} INCLUDING ALL)"
            ))
            .execute(&owner)
            .await
            .unwrap();
        }
        for query in [
            format!("GRANT USAGE ON SCHEMA {schema} TO tor_app"),
            format!("GRANT USAGE,SELECT ON ALL SEQUENCES IN SCHEMA {schema} TO tor_app"),
            format!(
                "GRANT SELECT,INSERT,UPDATE,DELETE ON ALL TABLES IN SCHEMA {schema} TO tor_app"
            ),
        ] {
            sqlx::query(&query).execute(&owner).await.unwrap();
        }
        let namespace = schema.clone();
        let app = PgPoolOptions::new()
            .max_connections(3)
            .acquire_timeout(Duration::from_secs(2))
            .after_connect(move |connection, _| {
                let schema = namespace.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path={schema},public"))
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("SET statement_timeout = '1s'")
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("SET lock_timeout = '75ms'")
                        .execute(&mut *connection)
                        .await?;
                    sqlx::query("SET TIME ZONE 'Asia/Kathmandu'")
                        .execute(connection)
                        .await?;
                    Ok(())
                })
            })
            .connect(&app_url)
            .await
            .unwrap();
        Self { owner, app, schema }
    }
    pub(super) async fn hour(&self) -> i64 {
        sqlx::query_scalar("SELECT EXTRACT(EPOCH FROM (date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'))::bigint")
            .fetch_one(&self.app).await.unwrap()
    }
    pub(super) async fn finish(self) {
        self.app.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.owner)
            .await
            .unwrap();
        self.owner.close().await;
    }
}

fn page() -> FlowPageview {
    FlowPageview {
        surface: RouteSurface::BookDetail,
        acquisition_hint: AcquisitionChannel::DirectOrUnknown,
    }
}
fn sample(hour: i64, count: usize) -> RollupDelta {
    let mut delta = RollupDelta::default();
    for _ in 0..count {
        assert_eq!(delta.record_observed_flow(page(), Some(hour)), Some(true));
    }
    delta
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci with canonical migrations"]
async fn original_hour_survives_retry_fencing_and_replay_after_legacy_tombstone() {
    let fixture = Fixture::new().await;
    let repo = PgRilRollups::new(fixture.app.clone());
    let hour = fixture.hour().await - 26 * 3_600;
    let delta = sample(hour, 3);
    let id = Uuid::new_v4().to_string();
    assert!(repo.enqueue(&id, &delta).await.unwrap());
    assert!(!repo.enqueue(&id, &sample(hour, 99)).await.unwrap());
    let stale = repo.claim_next().await.unwrap().unwrap();
    sqlx::query(
        "UPDATE ril_flush_batches SET leased_until=now()-interval '1 second' WHERE batch_id=$1",
    )
    .bind(&id)
    .execute(&fixture.app)
    .await
    .unwrap();
    let current = repo.claim_next().await.unwrap().unwrap();
    assert!(!repo.materialize(&stale).await.unwrap());
    assert!(repo.materialize(&current).await.unwrap());
    assert!(!repo.materialize(&current).await.unwrap());
    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT EXTRACT(EPOCH FROM bucket)::bigint,html_requests FROM ril_flow_observed_hourly",
    )
    .fetch_all(&fixture.app)
    .await
    .unwrap();
    assert_eq!(rows, vec![(hour, 3)]);
    // An unrelated server supplies another immutable UUID to the same hour.
    let second = Uuid::new_v4().to_string();
    repo.enqueue(&second, &sample(hour, 2)).await.unwrap();
    let next = repo.claim_next().await.unwrap().unwrap();
    assert!(repo.materialize(&next).await.unwrap());
    // Simulate old 90-day RIL tombstone removal; the 210-day projection receipt remains.
    sqlx::query("DELETE FROM ril_flush_flow_observed WHERE batch_id=$1")
        .bind(&id)
        .execute(&fixture.app)
        .await
        .unwrap();
    sqlx::query("DELETE FROM ril_flush_batches WHERE batch_id=$1")
        .bind(&id)
        .execute(&fixture.app)
        .await
        .unwrap();
    assert!(repo.enqueue(&id, &delta).await.unwrap());
    let replay = repo.claim_next().await.unwrap().unwrap();
    assert!(repo.materialize(&replay).await.unwrap());
    let saved: i64 = sqlx::query_scalar("SELECT html_requests FROM ril_flow_observed_hourly")
        .fetch_one(&fixture.app)
        .await
        .unwrap();
    assert_eq!(saved, 5);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM ril_flow_observed_receipts")
            .fetch_one(&fixture.app)
            .await
            .unwrap(),
        2
    );
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci with canonical migrations"]
async fn skew_expired_events_and_memory_losses_do_not_poison_other_counts() {
    let fixture = Fixture::new().await;
    let repo = PgRilRollups::new(fixture.app.clone());
    let now = fixture.hour().await;
    let mut delta = sample(now - 3_600, 2);
    delta.record_observed_flow(page(), Some(now - 181 * 86_400));
    delta.record_observed_flow(page(), Some(now + 48 * 3_600));
    delta.record_observed_flow(page(), None);
    let id = Uuid::new_v4().to_string();
    repo.enqueue(&id, &delta).await.unwrap();
    let claim = repo.claim_next().await.unwrap().unwrap();
    assert!(repo.materialize(&claim).await.unwrap());
    let discarded: i64 = sqlx::query_scalar(
        "SELECT discarded_events FROM ril_flow_observed_receipts WHERE batch_id=$1",
    )
    .bind(&id)
    .fetch_one(&fixture.app)
    .await
    .unwrap();
    assert_eq!(discarded, 3);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT SUM(html_requests)::bigint FROM ril_flow_observed_hourly"
        )
        .fetch_one(&fixture.app)
        .await
        .unwrap(),
        2
    );
    for (surface, source) in [(4, 0), (0, 4), (-1, 0)] {
        assert!(sqlx::query(
            "INSERT INTO ril_flow_observed_hourly VALUES(to_timestamp($1::bigint),$2,$3,1)"
        )
        .bind(now)
        .bind(surface as i16)
        .bind(source as i16)
        .execute(&fixture.app)
        .await
        .is_err());
    }
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci with canonical migrations"]
async fn every_period_is_exact_and_timezone_does_not_change_totals() {
    let fixture = Fixture::new().await;
    sqlx::query("INSERT INTO ril_flow_observed_hourly
        SELECT (date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')-n*interval '1 hour',
        s::smallint,a::smallint,(s+1)*(a+1)
        FROM generate_series(1,4320)n CROSS JOIN generate_series(0,3)s CROSS JOIN generate_series(0,3)a")
        .execute(&fixture.app).await.unwrap();
    let repo = PgFlowAudience::new(fixture.app.clone());
    for days in [1, 7, 30, 90] {
        let utc = repo.observed_report(days, "UTC").await.unwrap();
        let local = repo
            .observed_report(days, "Europe/Amsterdam")
            .await
            .unwrap();
        assert_eq!(utc.current.hours.len(), (days * 24) as usize);
        assert_eq!(utc.previous.saved_total, Some(i64::from(days) * 24 * 100));
        assert_eq!(utc.current.saved_total, utc.previous.saved_total);
        assert_eq!(local.current.surfaces, utc.current.surfaces);
        assert_eq!(local.current.sources, utc.current.sources);
        assert_eq!(local.current.recorded_hours, days as usize * 24);
    }
    // Real query at a fixed test-only anchor validates the repeated DST hour.
    let query = include_str!("../../src/repositories/audience/observed.sql")
        .replace("now()", "TIMESTAMPTZ '2026-10-25T04:00:00Z'");
    let rows = sqlx::query(&query)
        .bind(24_i32)
        .bind("Europe/Amsterdam")
        .fetch_all(&fixture.app)
        .await
        .unwrap();
    let offsets: Vec<i32> = rows
        .iter()
        .filter(|row| row.get::<String, _>("local_label") == "2026-10-25 02:00")
        .map(|row| row.get("offset_minutes"))
        .collect();
    assert_eq!(offsets, vec![120, 60]);
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci with canonical migrations"]
async fn missing_zero_retention_and_query_failure_are_distinct() {
    let fixture = Fixture::new().await;
    let hour = fixture.hour().await;
    let repo = PgFlowAudience::new(fixture.app.clone());
    assert_eq!(
        repo.observed_report(7, "UTC")
            .await
            .unwrap()
            .current
            .saved_total,
        None
    );
    sqlx::query("INSERT INTO ril_flow_observed_hourly VALUES(to_timestamp($1::bigint),0,3,0)")
        .bind(hour - 3_600)
        .execute(&fixture.app)
        .await
        .unwrap();
    let zero = repo.observed_report(7, "UTC").await.unwrap();
    assert_eq!(zero.current.saved_total, Some(0));
    assert_eq!(zero.current.recorded_hours, 1);
    assert_eq!(zero.current.hours[0].views, [None; 4]);
    sqlx::query("INSERT INTO ril_flow_observed_hourly VALUES(to_timestamp($1::bigint),0,3,11)")
        .bind(hour - 181 * 86_400)
        .execute(&fixture.app)
        .await
        .unwrap();
    let old = Uuid::new_v4().to_string();
    sqlx::query("INSERT INTO ril_flow_observed_receipts VALUES($1,now()-interval '211 days',5)")
        .bind(old)
        .execute(&fixture.app)
        .await
        .unwrap();
    PgRilRollups::new(fixture.app.clone())
        .prune_expired()
        .await
        .unwrap();
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM ril_flow_observed_hourly")
            .fetch_one(&fixture.app)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM ril_flow_observed_receipts")
            .fetch_one(&fixture.app)
            .await
            .unwrap(),
        0
    );
    let mut lock = fixture.owner.begin().await.unwrap();
    sqlx::query(&format!(
        "LOCK TABLE {}.ril_flow_observed_hourly IN ACCESS EXCLUSIVE MODE",
        fixture.schema
    ))
    .execute(&mut *lock)
    .await
    .unwrap();
    assert_eq!(
        repo.observed_report(7, "UTC").await,
        Err(FlowHourlyError::Unavailable)
    );
    lock.rollback().await.unwrap();
    assert_eq!(
        repo.observed_report(7, "UTC")
            .await
            .unwrap()
            .current
            .saved_total,
        Some(0)
    );
    for (days, zone) in [(8, "UTC"), (7, "not-a-zone")] {
        assert_eq!(
            repo.observed_report(days, zone).await,
            Err(FlowHourlyError::Invalid)
        );
    }
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci with canonical migrations"]
async fn migrated_guard_rejects_an_old_worker_completing_new_projection() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let pool = PgPool::connect(&url).await.unwrap();
    let id = Uuid::new_v4().to_string();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO ril_flush_batches(batch_id,bucket,flow_observed_discarded) VALUES($1,date_trunc('hour',now()),1)",
    )
    .bind(&id)
    .execute(&mut *tx)
    .await
    .unwrap();
    let error = sqlx::query("UPDATE ril_flush_batches SET status='done' WHERE batch_id=$1")
        .bind(&id)
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert_eq!(
        error
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("23514")
    );
    tx.rollback().await.unwrap();
    pool.close().await;
}
