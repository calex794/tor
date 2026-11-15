use super::*;
use crate::domain::exposure_health::{HealthCounters, HealthLease};
use sqlx::Row;

impl PgExposureHealth {
    pub async fn available(&self) -> bool {
        let check = async {
            let mut tx = self.pool.begin().await?;
            sqlx::query("SET TRANSACTION READ ONLY")
                .execute(&mut *tx)
                .await?;
            sqlx::query("SELECT set_config('statement_timeout','200ms',true),set_config('lock_timeout','25ms',true)")
                .execute(&mut *tx).await?;
            let ready: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM public._sqlx_migrations WHERE version=202611294684 AND success) AND to_regclass('public.flow_observation_epochs') IS NOT NULL AND to_regclass('public.flow_observation_health_hourly') IS NOT NULL"
            ).fetch_one(&mut *tx).await?;
            tx.commit().await?;
            Ok::<_, ExposureError>(ready)
        };
        matches!(
            tokio::time::timeout(Duration::from_millis(300), check).await,
            Ok(Ok(true))
        )
    }

    /// Subtract process age measured AFTER this returns. SQL latency must never
    /// shorten the known startup gap. No application wall clock is used.
    pub async fn clock_ms(&self) -> Result<i64, ExposureError> {
        bounded(async {
            let mut tx = self.pool.begin().await?;
            settings(&mut tx).await?;
            let now = now_ms(&mut tx).await?;
            tx.commit().await?;
            Ok(now)
        })
        .await
    }

    /// Atomic predecessor/gap/successor handoff. The random per-attempt key makes
    /// a timed-out successful commit discoverable; it is not a reader identity.
    pub async fn start_epoch(
        &self,
        registration: &str,
        predecessor: Option<i64>,
        gap_start_ms: i64,
        counters: HealthCounters,
    ) -> Result<HealthLease, ExposureError> {
        validate(counters)?;
        if registration.len() != 32
            || !registration
                .bytes()
                .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
            || gap_start_ms < 0
            || predecessor.is_some_and(|n| n <= 0)
        {
            return Err(ExposureError::Invalid);
        }
        bounded(async {
            let mut tx = self.pool.begin().await?;
            settings(&mut tx).await?;
            // Serialize retries even when the first attempt is still committing.
            sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,684))")
                .bind(registration).execute(&mut *tx).await?;
            let now = now_ms(&mut tx).await?;
            if let Some(row) = sqlx::query("SELECT epoch,CASE WHEN retired THEN 0 ELSE lease_until_ms END AS lease_until_ms FROM flow_observation_epochs WHERE registration_key=$1")
                .bind(registration).fetch_optional(&mut *tx).await?
            {
                let lease = HealthLease { epoch: row.try_get("epoch")?, clock_ms: now,
                    lease_until_ms: row.try_get("lease_until_ms")? };
                tx.commit().await?;
                return Ok(lease);
            }
            if gap_start_ms > now { return Err(ExposureError::Invalid); }
            let mut gap = gap_start_ms;
            let mut loss = counters.dropped > 0;
            let mut failure = counters.failed > 0;
            if let Some(old) = predecessor {
                let row = sqlx::query("SELECT * FROM flow_observation_epochs WHERE epoch=$1 FOR UPDATE")
                    .bind(old).fetch_one(&mut *tx).await?;
                if row.try_get::<bool, _>("retired")? || row.try_get::<i64, _>("confirmed_ms")? > now {
                    return Err(ExposureError::Expired);
                }
                (loss,failure)=deltas(&row,counters)?;
                gap = row.try_get("confirmed_ms")?;
                sqlx::query("UPDATE flow_observation_epochs SET retired=true,ended_ms=$2,pending_cut_ms=NULL,pending_target=NULL WHERE epoch=$1")
                    .bind(old).bind(now).execute(&mut *tx).await?;
            }
            let epoch: i64 = sqlx::query_scalar(
                "INSERT INTO flow_observation_epochs(registration_key,started_ms,confirmed_ms,lease_until_ms,dropped_seen,failed_seen) VALUES($1,$2,$2,$2+30000,$3,$4) RETURNING epoch"
            ).bind(registration).bind(now).bind(counters.dropped).bind(counters.failed).fetch_one(&mut *tx).await?;
            flag(&mut tx, epoch, gap.saturating_sub(902_000), now, loss, failure, true).await?;
            tx.commit().await?;
            Ok(HealthLease { epoch, clock_ms: now, lease_until_ms: now + 30_000 })
        }).await
    }

    /// Only after HTTP has stopped/drained and queued writes have settled.
    /// A disabled gate, expired lease, Drop, abort or panic is not that witness.
    pub async fn retire_after_ingress_drained(
        &self,
        epoch: i64,
        counters: HealthCounters,
    ) -> Result<(), ExposureError> {
        validate(counters)?;
        bounded(async {
            let mut tx = self.pool.begin().await?;
            settings(&mut tx).await?;
            let row = sqlx::query("SELECT * FROM flow_observation_epochs WHERE epoch=$1 FOR UPDATE")
                .bind(epoch).fetch_one(&mut *tx).await?;
            let now = now_ms(&mut tx).await?;
            if row.try_get::<bool, _>("retired")? { return Ok(()); }
            let confirmed: i64 = row.try_get("confirmed_ms")?;
            if confirmed > now { return Err(ExposureError::Unavailable); }
            let (lost, failed) = deltas(&row, counters)?;
            flag(&mut tx, epoch, confirmed.saturating_sub(902_000), now, lost, failed, false).await?;
            sqlx::query("UPDATE flow_observation_epochs SET retired=true,ended_ms=$2,dropped_seen=$3,failed_seen=$4,pending_cut_ms=NULL,pending_target=NULL WHERE epoch=$1")
                .bind(epoch).bind(now).bind(counters.dropped).bind(counters.failed).execute(&mut *tx).await?;
            tx.commit().await?;
            Ok(())
        }).await
    }

    pub async fn prune(&self) -> Result<u64, ExposureError> {
        bounded(async {
            let mut tx = self.pool.begin().await?;
            settings(&mut tx).await?;
            let removed = sqlx::query("DELETE FROM flow_observation_health_hourly WHERE (epoch,bucket_ms) IN(SELECT epoch,bucket_ms FROM flow_observation_health_hourly WHERE bucket_ms<floor(extract(epoch FROM now()-interval '181 days')*1000)::bigint ORDER BY bucket_ms,epoch LIMIT 1000 FOR UPDATE SKIP LOCKED)")
                .execute(&mut *tx).await?.rows_affected();
            sqlx::query("DELETE FROM flow_observation_epochs e WHERE epoch IN(SELECT x.epoch FROM flow_observation_epochs x WHERE x.retired AND x.ended_ms<floor(extract(epoch FROM now()-interval '181 days')*1000)::bigint AND NOT EXISTS(SELECT 1 FROM flow_observation_health_hourly h WHERE h.epoch=x.epoch) ORDER BY x.ended_ms,x.epoch LIMIT 1000 FOR UPDATE SKIP LOCKED)")
                .execute(&mut *tx).await?;
            tx.commit().await?;
            Ok(removed)
        }).await
    }
}
pub(super) async fn now_ms(tx: &mut Transaction<'_, Postgres>) -> Result<i64, ExposureError> {
    Ok(
        sqlx::query_scalar("SELECT floor(extract(epoch FROM clock_timestamp())*1000)::bigint")
            .fetch_one(&mut **tx)
            .await?,
    )
}
pub(super) fn validate(c: HealthCounters) -> Result<(), ExposureError> {
    if c.dropped < 0 || c.failed < 0 {
        Err(ExposureError::Invalid)
    } else {
        Ok(())
    }
}
pub(super) fn deltas(
    row: &sqlx::postgres::PgRow,
    c: HealthCounters,
) -> Result<(bool, bool), ExposureError> {
    let dropped: i64 = row.try_get("dropped_seen")?;
    let failed: i64 = row.try_get("failed_seen")?;
    if c.dropped < dropped || c.failed < failed {
        return Err(ExposureError::Invalid);
    }
    Ok((c.dropped > dropped, c.failed > failed))
}
pub(super) async fn flag(
    tx: &mut Transaction<'_, Postgres>,
    epoch: i64,
    from: i64,
    through: i64,
    lost: bool,
    failed: bool,
    uncertain: bool,
) -> Result<(), ExposureError> {
    if !lost && !failed && !uncertain {
        return Ok(());
    }
    // Include the bucket containing floor(ms), a lower bound on the DB instant.
    sqlx::query(
        r#"INSERT INTO flow_observation_health_hourly(epoch,bucket_ms,lost,failed,uncertain)
 SELECT $1,h,$4,$5,$6 FROM generate_series(
   (greatest($2,0::bigint,$3-15638400000)/3600000)*3600000,
   ($3::bigint/3600000)*3600000,3600000::bigint) h
 ON CONFLICT(epoch,bucket_ms) DO UPDATE SET
   lost=flow_observation_health_hourly.lost OR EXCLUDED.lost,
   failed=flow_observation_health_hourly.failed OR EXCLUDED.failed,
   uncertain=flow_observation_health_hourly.uncertain OR EXCLUDED.uncertain"#,
    )
    .bind(epoch)
    .bind(from)
    .bind(through)
    .bind(lost)
    .bind(failed)
    .bind(uncertain)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
