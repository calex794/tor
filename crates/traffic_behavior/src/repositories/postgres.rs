//! PostgreSQL RIL-06 batch staging, exclusive leases and exact-once rollups.
//! No SQL runs in the HTTP request path. The schema contains only finite codes.
use crate::domain::RollupDelta;
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgRilRollups {
    pool: PgPool,
}

#[derive(Clone, Debug)]
pub struct BatchClaim {
    pub id: String,
    pub epoch: i64,
}

impl PgRilRollups {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// RIL-10 read-only FinOps evidence, bounded to 30 days of retained hourly data.
    /// Returns at most 15 routes x 6 statuses x 4 latency buckets (360 rows).
    /// This is a histogram of observed HTTP response times, NOT measured DB calls,
    /// CPU time, monetary charges, or per-request diagnostic information.
    pub async fn read_cost_buckets(
        &self,
        days: u16,
    ) -> Result<Vec<crate::domain::RouteCount>, sqlx::Error> {
        if !(1..=30).contains(&days) {
            return Err(sqlx::Error::Protocol(
                "RIL cost history window must be between 1 and 30 days".into(),
            ));
        }
        let rows = sqlx::query(
            "SELECT route_code,status_group,latency_group,
                    LEAST(SUM(request_count),9223372036854775807)::bigint AS request_count
               FROM ril_route_hourly
              WHERE bucket >= date_trunc('hour',now()) - ($1::integer * INTERVAL '1 day')
                AND bucket <= date_trunc('hour',now())
              GROUP BY route_code,status_group,latency_group
              ORDER BY route_code,status_group,latency_group",
        )
        .bind(i32::from(days))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(crate::domain::RouteCount {
                    route: row.try_get("route_code")?,
                    status: row.try_get("status_group")?,
                    latency: row.try_get("latency_group")?,
                    count: row.try_get("request_count")?,
                })
            })
            .collect()
    }

    /// RIL-10: measured SQLx completion counts for bounded route groups.
    /// These are application-side observed query completions, NOT server CPU
    /// time or money. The two counters are not necessarily complete for
    /// spawned background tasks; observed_requests states coverage explicitly.
    pub async fn read_sql_costs(&self, days: u16) -> Result<Vec<crate::domain::SqlCostCount>, sqlx::Error> {
        if !(1..=30).contains(&days) {
            return Err(sqlx::Error::Protocol("RIL SQLx window must be 1..=30 days".into()));
        }
        let rows = sqlx::query(
            "SELECT route_code,
                    LEAST(SUM(observed_requests),9223372036854775807)::bigint AS observed_requests,
                    LEAST(SUM(observed_queries),9223372036854775807)::bigint AS observed_queries,
                    LEAST(SUM(sqlx_elapsed_us),9223372036854775807)::bigint AS sqlx_elapsed_us
             FROM ril_sql_cost_hourly
             WHERE bucket >= date_trunc('hour',now())-($1::integer * INTERVAL '1 day')
               AND bucket <= date_trunc('hour',now())
             GROUP BY route_code ORDER BY route_code",
        )
        .bind(i32::from(days))
        .fetch_all(&self.pool)
        .await?;
        rows.into_iter()
            .map(|row| Ok(crate::domain::SqlCostCount {
                route: row.try_get("route_code")?,
                observed_requests: row.try_get("observed_requests")?,
                observed_queries: row.try_get("observed_queries")?,
                sqlx_elapsed_us: row.try_get("sqlx_elapsed_us")?,
            }))
            .collect()
    }

    /// Stage a single immutable, identified batch atomically. Replays with the
    /// same UUID never reinsert its counters even if a prior commit response
    /// was lost. Distinct process restarts receive distinct UUIDs.
    pub async fn enqueue(&self, id: &str, delta: &RollupDelta) -> Result<bool, sqlx::Error> {
        if delta.is_empty() {
            return Ok(false);
        }
        if id.len() != 36 {
            return Err(sqlx::Error::Protocol("RIL batch ID must be UUIDv4".into()));
        }
        let mut tx = self.pool.begin().await?;
        let inserted = sqlx::query_scalar::<_, String>(
            "INSERT INTO ril_flush_batches(batch_id,bucket)
             VALUES($1,date_trunc('hour',now()))
             ON CONFLICT(batch_id) DO NOTHING RETURNING batch_id",
        )
        .bind(id)
        .fetch_optional(&mut *tx)
        .await?;
        if inserted.is_none() {
            tx.commit().await?;
            return Ok(false);
        }

        let route_rows = delta.route_rows();
        let routes: Vec<i16> = route_rows.iter().map(|r| r.route).collect();
        let statuses: Vec<i16> = route_rows.iter().map(|r| r.status).collect();
        let latencies: Vec<i16> = route_rows.iter().map(|r| r.latency).collect();
        let counts: Vec<i64> = route_rows.iter().map(|r| r.count).collect();
        sqlx::query(
            "INSERT INTO ril_flush_routes
               (batch_id,route_code,status_group,latency_group,request_count)
             SELECT $1, t.route_code,t.status_group,t.latency_group,t.n
             FROM unnest($2::smallint[],$3::smallint[],$4::smallint[],$5::bigint[])
                  AS t(route_code,status_group,latency_group,n)",
        )
        .bind(id)
        .bind(routes)
        .bind(statuses)
        .bind(latencies)
        .bind(counts)
        .execute(&mut *tx)
        .await?;

        let transition_rows = delta.transition_rows();
        let from: Vec<i16> = transition_rows.iter().map(|r| r.from).collect();
        let to: Vec<i16> = transition_rows.iter().map(|r| r.to).collect();
        let gaps: Vec<i16> = transition_rows.iter().map(|r| r.gap).collect();
        let counts: Vec<i64> = transition_rows.iter().map(|r| r.count).collect();
        sqlx::query(
            "INSERT INTO ril_flush_transitions
               (batch_id,from_code,to_code,gap_group,transition_count)
             SELECT $1,t.from_code,t.to_code,t.gap_group,t.n
             FROM unnest($2::smallint[],$3::smallint[],$4::smallint[],$5::bigint[])
                  AS t(from_code,to_code,gap_group,n)",
        )
        .bind(id)
        .bind(from)
        .bind(to)
        .bind(gaps)
        .bind(counts)
        .execute(&mut *tx)
        .await?;
        // SQLx telemetry shares RIL-06's immutable batch ID and transaction:
        // a lost commit response cannot apply these counts a second time.
        let sql_rows = delta.sql_rows();
        let sql_routes: Vec<i16> = sql_rows.iter().map(|r| r.route).collect();
        let observed: Vec<i64> = sql_rows.iter().map(|r| r.observed_requests).collect();
        let queries: Vec<i64> = sql_rows.iter().map(|r| r.observed_queries).collect();
        let elapsed: Vec<i64> = sql_rows.iter().map(|r| r.sqlx_elapsed_us).collect();
        sqlx::query(
            "INSERT INTO ril_flush_sql_costs
                 (batch_id,route_code,observed_requests,observed_queries,sqlx_elapsed_us)
             SELECT $1,t.route_code,t.observed_requests,t.observed_queries,t.sqlx_elapsed_us
             FROM unnest($2::smallint[],$3::bigint[],$4::bigint[],$5::bigint[])
                 AS t(route_code,observed_requests,observed_queries,sqlx_elapsed_us)",
        )
        .bind(id)
        .bind(sql_routes)
        .bind(observed)
        .bind(queries)
        .bind(elapsed)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(true)
    }

    /// Recover unmaterialized batches after crash/lease expiry. Concurrent
    /// workers cannot claim the same batch; epoch fences stale claim holders.
    pub async fn claim_next(&self) -> Result<Option<BatchClaim>, sqlx::Error> {
        let row = sqlx::query(
            "WITH candidate AS (
               SELECT batch_id FROM ril_flush_batches
                WHERE attempts < 16 AND (
                    (status='pending' AND run_after<=now())
                    OR (status='running' AND leased_until < now())
                )
                ORDER BY run_after,enqueued_at,batch_id
                FOR UPDATE SKIP LOCKED LIMIT 1
             )
             UPDATE ril_flush_batches b
             SET status='running',lease_epoch=b.lease_epoch+1,
                 attempts=b.attempts+1,
                 leased_until=now()+INTERVAL '90 seconds'
             FROM candidate c WHERE b.batch_id=c.batch_id
             RETURNING b.batch_id,b.lease_epoch",
        )
        .fetch_optional(&self.pool)
        .await?;
        row.map(|row| {
            Ok(BatchClaim {
                id: row.try_get("batch_id")?,
                epoch: row.try_get("lease_epoch")?,
            })
        })
        .transpose()
    }

    /// The rollup updates and 'done' state occur in the SAME transaction.
    /// A process crash either rolls everything back or applies once.
    pub async fn materialize(&self, claim: &BatchClaim) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let active = sqlx::query_scalar::<_, String>(
            "SELECT batch_id FROM ril_flush_batches
              WHERE batch_id=$1 AND lease_epoch=$2 AND status='running'
                AND leased_until>now() FOR UPDATE",
        )
        .bind(&claim.id)
        .bind(claim.epoch)
        .fetch_optional(&mut *tx)
        .await?;
        if active.is_none() {
            return Ok(false);
        }
        sqlx::query(
            "INSERT INTO ril_route_hourly
               (bucket,route_code,status_group,latency_group,request_count)
             SELECT b.bucket,r.route_code,r.status_group,r.latency_group,r.request_count
               FROM ril_flush_routes r JOIN ril_flush_batches b ON b.batch_id=r.batch_id
              WHERE r.batch_id=$1
             ON CONFLICT(bucket,route_code,status_group,latency_group)
             DO UPDATE SET request_count=
               ril_route_hourly.request_count+EXCLUDED.request_count",
        )
        .bind(&claim.id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO ril_transition_hourly
               (bucket,from_code,to_code,gap_group,transition_count)
             SELECT b.bucket,r.from_code,r.to_code,r.gap_group,r.transition_count
               FROM ril_flush_transitions r
               JOIN ril_flush_batches b ON b.batch_id=r.batch_id
              WHERE r.batch_id=$1
             ON CONFLICT(bucket,from_code,to_code,gap_group)
             DO UPDATE SET transition_count=
               ril_transition_hourly.transition_count+EXCLUDED.transition_count",
        )
        .bind(&claim.id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO ril_daily_rollup(day,route_code,requests,http_errors,slow_requests)
             SELECT b.bucket::date,r.route_code,
                    SUM(r.request_count),
                    SUM(CASE WHEN r.status_group IN (3,4)
                             THEN r.request_count ELSE 0 END),
                    SUM(CASE WHEN r.latency_group=3
                             THEN r.request_count ELSE 0 END)
               FROM ril_flush_routes r JOIN ril_flush_batches b ON b.batch_id=r.batch_id
              WHERE r.batch_id=$1
              GROUP BY b.bucket::date,r.route_code
             ON CONFLICT(day,route_code)
             DO UPDATE SET
               requests=ril_daily_rollup.requests+EXCLUDED.requests,
               http_errors=ril_daily_rollup.http_errors+EXCLUDED.http_errors,
               slow_requests=ril_daily_rollup.slow_requests+EXCLUDED.slow_requests",
        )
        .bind(&claim.id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO ril_sql_cost_hourly
                 (bucket,route_code,observed_requests,observed_queries,sqlx_elapsed_us)
             SELECT b.bucket,s.route_code,s.observed_requests,
                    s.observed_queries,s.sqlx_elapsed_us
               FROM ril_flush_sql_costs s
               JOIN ril_flush_batches b ON b.batch_id=s.batch_id
              WHERE s.batch_id=$1
             ON CONFLICT(bucket,route_code)
             DO UPDATE SET
               observed_requests=ril_sql_cost_hourly.observed_requests+EXCLUDED.observed_requests,
               observed_queries=ril_sql_cost_hourly.observed_queries+EXCLUDED.observed_queries,
               sqlx_elapsed_us=ril_sql_cost_hourly.sqlx_elapsed_us+EXCLUDED.sqlx_elapsed_us",
        )
        .bind(&claim.id)
        .execute(&mut *tx)
        .await?;
        let updated = sqlx::query(
            "UPDATE ril_flush_batches
             SET status='done',finished_at=now(),leased_until=NULL
             WHERE batch_id=$1 AND lease_epoch=$2 AND status='running'",
        )
        .bind(&claim.id)
        .bind(claim.epoch)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Ok(false);
        }
        tx.commit().await?;
        Ok(true)
    }

    /// Operator/worker retry after a DB failure, protected by lease epoch.
    /// Lost leases must not reset the new worker's attempt.
    pub async fn retry(&self, claim: &BatchClaim) -> Result<bool, sqlx::Error> {
        let updated = sqlx::query(
            "UPDATE ril_flush_batches
                SET status=CASE WHEN attempts<16 THEN 'pending' ELSE 'dead' END,
                    leased_until=NULL,run_after=now()+INTERVAL '5 seconds'
              WHERE batch_id=$1 AND lease_epoch=$2 AND status='running'",
        )
        .bind(&claim.id)
        .bind(claim.epoch)
        .execute(&self.pool)
        .await?;
        Ok(updated.rows_affected() == 1)
    }

    /// All expiration is outside the request path. Batch tombstones are kept
    /// longer than hourly aggregates to avoid reapplication within retention.
    pub async fn prune_expired(&self) -> Result<(), sqlx::Error> {
        sqlx::query(
            "DELETE FROM ril_flush_batches
              WHERE status='done' AND finished_at < now()-INTERVAL '90 days'",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "DELETE FROM ril_route_hourly
              WHERE bucket < date_trunc('hour',now()-INTERVAL '30 days')",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "DELETE FROM ril_sql_cost_hourly
              WHERE bucket < date_trunc('hour',now()-INTERVAL '30 days')",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "DELETE FROM ril_transition_hourly
              WHERE bucket < date_trunc('hour',now()-INTERVAL '30 days')",
        )
        .execute(&self.pool)
        .await?;
        sqlx::query(
            "DELETE FROM ril_daily_rollup
              WHERE day < (now()-INTERVAL '90 days')::date",
        )
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
