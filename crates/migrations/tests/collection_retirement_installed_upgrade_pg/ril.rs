//! A mixed old producer batch keeps its anonymous route/SQL/HTML component.
use super::fixtures::{count, run, text, Fixture, TestResult};
use sqlx::PgPool;

pub async fn seed(pool: &PgPool, batch: &str) -> TestResult {
    stage(pool, batch, 11).await?;
    run(
        pool,
        "INSERT INTO ril_flow_hourly(bucket,surface_code,acquisition_hint,html_requests,
             opted_in_html_requests,observed_opted_in_visit_starts)
         VALUES(date_trunc('hour',now()),1,0,20,5,2);
         INSERT INTO ril_transition_hourly(bucket,from_code,to_code,gap_group,transition_count)
         VALUES(date_trunc('hour',now()),1,2,0,2)",
    )
    .await
}

async fn stage(pool: &PgPool, batch: &str, requests: i64) -> TestResult {
    run(pool, &format!(
        "INSERT INTO ril_flush_batches(batch_id,bucket) VALUES('{batch}',date_trunc('hour',now()));
         INSERT INTO ril_flush_routes(batch_id,route_code,status_group,latency_group,request_count)
         VALUES('{batch}',1,2,0,{requests});
         INSERT INTO ril_flush_sql_costs(batch_id,route_code,observed_requests,observed_queries,sqlx_elapsed_us)
         VALUES('{batch}',1,{requests},{requests}*2,{requests}*100);
         INSERT INTO ril_flush_flow(batch_id,surface_code,acquisition_hint,html_requests,
             opted_in_html_requests,observed_opted_in_visit_starts)
         VALUES('{batch}',1,0,{requests},5,2);
         INSERT INTO ril_flush_transitions(batch_id,from_code,to_code,gap_group,transition_count)
         VALUES('{batch}',1,2,0,4)"
    )).await
}

async fn materialize(pool: &PgPool, batch: &str) -> TestResult<bool> {
    let mut tx = pool.begin().await?;
    let status: String =
        sqlx::query_scalar("SELECT status FROM ril_flush_batches WHERE batch_id=$1 FOR UPDATE")
            .bind(batch)
            .fetch_one(&mut *tx)
            .await?;
    if status == "done" {
        tx.rollback().await?;
        return Ok(false);
    }
    assert_eq!(status, "pending");
    // These INSERT/UPSERT shapes are the existing G batch materializer. Its
    // durable receipt, not a new analytics event ledger, prevents replay.
    sqlx::raw_sql(&format!(
        "INSERT INTO ril_route_hourly(bucket,route_code,status_group,latency_group,request_count)
         SELECT b.bucket,r.route_code,r.status_group,r.latency_group,r.request_count
         FROM ril_flush_routes r JOIN ril_flush_batches b USING(batch_id) WHERE batch_id='{batch}'
         ON CONFLICT(bucket,route_code,status_group,latency_group) DO UPDATE SET
           request_count=ril_route_hourly.request_count+EXCLUDED.request_count;
         INSERT INTO ril_sql_cost_hourly(bucket,route_code,observed_requests,observed_queries,sqlx_elapsed_us)
         SELECT b.bucket,r.route_code,r.observed_requests,r.observed_queries,r.sqlx_elapsed_us
         FROM ril_flush_sql_costs r JOIN ril_flush_batches b USING(batch_id) WHERE batch_id='{batch}'
         ON CONFLICT(bucket,route_code) DO UPDATE SET
           observed_requests=ril_sql_cost_hourly.observed_requests+EXCLUDED.observed_requests,
           observed_queries=ril_sql_cost_hourly.observed_queries+EXCLUDED.observed_queries,
           sqlx_elapsed_us=ril_sql_cost_hourly.sqlx_elapsed_us+EXCLUDED.sqlx_elapsed_us;
         INSERT INTO ril_transition_hourly(bucket,from_code,to_code,gap_group,transition_count)
         SELECT b.bucket,r.from_code,r.to_code,r.gap_group,r.transition_count
         FROM ril_flush_transitions r JOIN ril_flush_batches b USING(batch_id) WHERE batch_id='{batch}'
         ON CONFLICT(bucket,from_code,to_code,gap_group) DO UPDATE SET
           transition_count=ril_transition_hourly.transition_count+EXCLUDED.transition_count;
         INSERT INTO ril_flow_hourly(bucket,surface_code,acquisition_hint,html_requests,
             opted_in_html_requests,observed_opted_in_visit_starts)
         SELECT b.bucket,r.surface_code,r.acquisition_hint,r.html_requests,
             r.opted_in_html_requests,r.observed_opted_in_visit_starts
         FROM ril_flush_flow r JOIN ril_flush_batches b USING(batch_id) WHERE batch_id='{batch}'
         ON CONFLICT(bucket,surface_code,acquisition_hint) DO UPDATE SET
           html_requests=ril_flow_hourly.html_requests+EXCLUDED.html_requests,
           opted_in_html_requests=ril_flow_hourly.opted_in_html_requests+EXCLUDED.opted_in_html_requests,
           observed_opted_in_visit_starts=ril_flow_hourly.observed_opted_in_visit_starts+
             EXCLUDED.observed_opted_in_visit_starts;
         UPDATE ril_flush_batches SET status='done',finished_at=now() WHERE batch_id='{batch}'"
    )).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(true)
}

pub async fn mixed_batches(pool: &PgPool, fixture: &Fixture) -> TestResult {
    let new_batch = text(pool, "SELECT gen_random_uuid()::text").await?;
    stage(pool, &new_batch, 5).await?;
    assert_eq!(
        text(
            pool,
            &format!(
        "SELECT concat_ws('|',html_requests,opted_in_html_requests,observed_opted_in_visit_starts)
         FROM ril_flush_flow WHERE batch_id='{new_batch}'"
    )
        )
        .await?,
        "5|0|0"
    );
    assert_eq!(
        count(
            pool,
            &format!("SELECT count(*) FROM ril_flush_transitions WHERE batch_id='{new_batch}'")
        )
        .await?,
        0
    );
    for batch in [&fixture.ril_batch, &new_batch] {
        assert!(materialize(pool, batch).await?);
        assert!(!materialize(pool, batch).await?);
    }
    assert_eq!(text(pool,
        "SELECT concat_ws('|',html_requests,opted_in_html_requests,observed_opted_in_visit_starts)
         FROM ril_flow_hourly WHERE surface_code=1 AND acquisition_hint=0"
    ).await?, "36|5|2");
    assert_eq!(
        count(
            pool,
            "SELECT request_count FROM ril_route_hourly
         WHERE route_code=1 AND status_group=2 AND latency_group=0"
        )
        .await?,
        16
    );
    assert_eq!(
        text(
            pool,
            "SELECT concat_ws('|',observed_requests,observed_queries,sqlx_elapsed_us)
         FROM ril_sql_cost_hourly WHERE route_code=1"
        )
        .await?,
        "16|32|1600"
    );
    assert_eq!(
        count(
            pool,
            "SELECT transition_count FROM ril_transition_hourly
         WHERE from_code=1 AND to_code=2 AND gap_group=0"
        )
        .await?,
        2,
        "old anonymous historical transition stays frozen"
    );
    Ok(())
}
