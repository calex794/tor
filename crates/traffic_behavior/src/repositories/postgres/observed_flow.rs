//! Same fenced RIL transaction, separate non-identifying event-hour projection.
use crate::domain::RollupDelta;
use sqlx::{Postgres, Transaction};

pub(super) async fn stage(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
    delta: &RollupDelta,
) -> Result<(), sqlx::Error> {
    let rows = delta.observed_flow_rows();
    let hours: Vec<i64> = rows.iter().map(|r| r.hour_epoch).collect();
    let surfaces: Vec<i16> = rows.iter().map(|r| r.surface).collect();
    let sources: Vec<i16> = rows.iter().map(|r| r.acquisition_hint).collect();
    let counts: Vec<i64> = rows.iter().map(|r| r.html_requests).collect();
    sqlx::query(
        "INSERT INTO ril_flush_flow_observed
        (batch_id,bucket,surface_code,acquisition_hint,html_requests)
        SELECT $1,to_timestamp(t.hour_epoch),t.surface,t.source,t.n
        FROM unnest($2::bigint[],$3::smallint[],$4::smallint[],$5::bigint[])
             AS t(hour_epoch,surface,source,n)",
    )
    .bind(id)
    .bind(hours)
    .bind(surfaces)
    .bind(sources)
    .bind(counts)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn materialize(
    tx: &mut Transaction<'_, Postgres>,
    id: &str,
) -> Result<(), sqlx::Error> {
    // Receipt and counters commit together; a missing/old lease never reaches here.
    let inserted = sqlx::query_scalar::<_, String>(
        "INSERT INTO ril_flow_observed_receipts(batch_id,discarded_events)
         SELECT b.batch_id,b.flow_observed_discarded + COALESCE((
             SELECT SUM(s.html_requests)::bigint FROM ril_flush_flow_observed s
             WHERE s.batch_id=b.batch_id AND (
                 s.bucket < (date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                     - interval '180 days'
                 OR s.bucket > (date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
                     + interval '1 hour')),0)
         FROM ril_flush_batches b WHERE b.batch_id=$1
           AND (b.flow_observed_discarded>0 OR EXISTS (
               SELECT 1 FROM ril_flush_flow_observed s WHERE s.batch_id=b.batch_id))
         ON CONFLICT(batch_id) DO NOTHING RETURNING batch_id",
    )
    .bind(id)
    .fetch_optional(&mut **tx)
    .await?;
    if inserted.is_none() {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO ril_flow_observed_hourly
        (bucket,surface_code,acquisition_hint,html_requests)
        SELECT bucket,surface_code,acquisition_hint,html_requests
        FROM ril_flush_flow_observed WHERE batch_id=$1
          AND bucket >= (date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
              - interval '180 days'
          AND bucket <= (date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
              + interval '1 hour'
        ON CONFLICT(bucket,surface_code,acquisition_hint) DO UPDATE SET
        html_requests=ril_flow_observed_hourly.html_requests+EXCLUDED.html_requests",
    )
    .bind(id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn prune(pool: &sqlx::PgPool) -> Result<(), sqlx::Error> {
    // Bounded work per maintenance cycle; receipts stay longer than visible data.
    sqlx::query(
        "DELETE FROM ril_flow_observed_hourly WHERE
        (bucket,surface_code,acquisition_hint) IN (
          SELECT bucket,surface_code,acquisition_hint FROM ril_flow_observed_hourly
          WHERE bucket < (date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')
              - interval '180 days'
          ORDER BY bucket,surface_code,acquisition_hint LIMIT 10000)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "DELETE FROM ril_flow_observed_receipts WHERE batch_id IN (
        SELECT batch_id FROM ril_flow_observed_receipts
        WHERE materialized_at < now()-interval '210 days'
        ORDER BY materialized_at,batch_id LIMIT 10000)",
    )
    .execute(pool)
    .await?;
    Ok(())
}
