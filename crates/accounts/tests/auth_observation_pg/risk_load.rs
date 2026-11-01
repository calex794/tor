//! Test-only synthetic history; transaction rollback leaves no bulk fixtures.
use tor_accounts::repositories::auth_risk_signals::{AuthRiskSignal, AUTH_RISK_SIGNALS_SQL};

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn a_09_risk_query_uses_time_index_and_handles_large_history() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated test database");
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1"));
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO auth_login_observations
         (observed_at,outcome,target_key,source_key,agent_family)
         SELECT now()-INTERVAL '2 days', 'success',
                decode(md5(i::text)||md5(i::text),'hex'),
                decode(md5((i%100)::text)||md5((i%100)::text),'hex'), 'unknown'
         FROM generate_series(1,100000) i",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO auth_login_observations
         (outcome,target_key,source_key,agent_family)
         SELECT 'success', decode(md5(i::text)||md5(i::text),'hex'),
                decode(md5((i%100)::text)||md5((i%100)::text),'hex'), 'unknown'
         FROM generate_series(1,1000) i",
    )
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("ANALYZE auth_login_observations")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SET LOCAL work_mem = '2MB'")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("SET LOCAL statement_timeout = '2s'")
        .execute(&mut *tx)
        .await
        .unwrap();
    let plan: Vec<String> = sqlx::query_scalar(&format!(
        "EXPLAIN (ANALYZE, BUFFERS, FORMAT TEXT) {AUTH_RISK_SIGNALS_SQL}"
    ))
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    let plan = plan.join("\n");
    assert!(
        plan.contains("auth_login_observations_time"),
        "sparse recent window must use the existing time index: {plan}"
    );
    println!("A-09 synthetic 100k history + 1k recent, work_mem=2MB:\n{plan}");
    let rows: Vec<AuthRiskSignal> = sqlx::query_as(AUTH_RISK_SIGNALS_SQL)
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert!(rows.len() <= 50);
    assert!(rows
        .iter()
        .all(|row| row.bad_passwords + row.unknown_accounts >= 10));
    tx.rollback().await.unwrap();
}
