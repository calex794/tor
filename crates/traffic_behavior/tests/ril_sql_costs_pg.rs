//! Real PG18 evidence: SQLx samples are staged and materialized atomically.
//! No query text, user ID, IP, cookie, request ID or raw route is stored.
use sqlx::{postgres::PgPoolOptions, Row};
use std::time::Duration;
use tor_traffic_behavior::{
    domain::{RollupDelta, SqlCostEvent},
    repositories::PgRilRollups,
    RouteCode,
};
use uuid::Uuid;

#[tokio::test]
#[ignore = "isolated PostgreSQL 18 with RIL SQLx migration required"]
async fn atomic_sql_costs_are_idempotent_and_bounded_in_real_pg() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG CI URL");
    assert!(url.ends_with("/tor_ci"), "do not write to production");
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&url)
        .await
        .unwrap();
    let repo = PgRilRollups::new(pool.clone());
    assert!(repo.read_sql_costs(0).await.is_err());
    assert!(repo.read_sql_costs(31).await.is_err());

    let baseline = repo.read_sql_costs(1).await.unwrap();
    let old = baseline.iter().find(|row| row.route == RouteCode::Books as i16);
    let old_requests = old.map_or(0, |r| r.observed_requests);
    let old_queries = old.map_or(0, |r| r.observed_queries);
    let old_elapsed = old.map_or(0, |r| r.sqlx_elapsed_us);

    let mut delta = RollupDelta::default();
    delta.record_sql(SqlCostEvent {
        route: RouteCode::Books,
        observed_queries: 3,
        sqlx_elapsed_us: 4567,
    });
    delta.record_sql(SqlCostEvent {
        route: RouteCode::Books,
        observed_queries: 0,
        sqlx_elapsed_us: 0,
    });
    let id = Uuid::new_v4().to_string();
    assert!(repo.enqueue(&id, &delta).await.unwrap());
    assert!(!repo.enqueue(&id, &delta).await.unwrap());
    let claim = repo.claim_next().await.unwrap().unwrap();
    assert_eq!(claim.id, id);
    assert!(repo.materialize(&claim).await.unwrap());
    assert!(!repo.materialize(&claim).await.unwrap());
    let after = repo.read_sql_costs(1).await.unwrap();
    assert!(after.len() <= 15);
    let value = after.iter().find(|r| r.route == RouteCode::Books as i16).unwrap();
    assert_eq!(value.observed_requests - old_requests, 2);
    assert_eq!(value.observed_queries - old_queries, 3);
    assert_eq!(value.sqlx_elapsed_us - old_elapsed, 4567);

    let stored: (i64, i64, i64) = sqlx::query_as(
        "SELECT observed_requests,observed_queries,sqlx_elapsed_us
         FROM ril_flush_sql_costs WHERE batch_id=$1 AND route_code=$2",
    )
    .bind(&id)
    .bind(RouteCode::Books as i16)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored, (2, 3, 4567));
    let columns: Vec<String> = sqlx::query(
        "SELECT column_name FROM information_schema.columns
         WHERE table_schema='public' AND table_name='ril_sql_cost_hourly'",
    )
    .fetch_all(&pool)
    .await
    .unwrap()
    .iter()
    .map(|r| r.try_get::<String, _>("column_name").unwrap())
    .collect();
    for forbidden in ["path", "sql", "user_id", "ip", "session_id", "request_id", "query_text"] {
        assert!(!columns.iter().any(|col| col == forbidden), "unexpected PII column");
    }
    pool.close().await;
}
