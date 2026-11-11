use super::lifecycle::{deltas, flag, now_ms, validate};
use super::*;
use crate::domain::exposure_health::{HealthCounters, HealthCut, HealthLease};
use sqlx::Row;

impl PgExposureHealth {
    /// Load the issued queue counter only AFTER this returns. Never retain a
    /// connection while waiting for registration settlement.
    pub async fn begin_checkpoint(&self, epoch: i64) -> Result<HealthCut, ExposureError> {
        bounded(async {
            let mut tx = self.pool.begin().await?;
            settings(&mut tx).await?;
            let row = sqlx::query("SELECT * FROM flow_observation_epochs WHERE epoch=$1 FOR UPDATE")
                .bind(epoch).fetch_one(&mut *tx).await?;
            let now = now_ms(&mut tx).await?;
            live(&row, now)?;
            let pending: Option<i64> = row.try_get("pending_cut_ms")?;
            let cut = pending.unwrap_or(now);
            sqlx::query("UPDATE flow_observation_epochs SET lease_until_ms=$2+30000,pending_cut_ms=$3,pending_target=coalesce(pending_target,0) WHERE epoch=$1")
                .bind(epoch).bind(now).bind(cut).execute(&mut *tx).await?;
            tx.commit().await?;
            Ok(HealthCut { lease: HealthLease { epoch, clock_ms: now, lease_until_ms: now+30_000 }, cut_ms: cut })
        }).await
    }

    /// Pending settlement renews only a LIVE lease without confirming the gap.
    /// Registration has its own settled counter; callback failures are separate.
    pub async fn finish_checkpoint(
        &self,
        cut: HealthCut,
        target: i64,
        settled: i64,
        counters: HealthCounters,
    ) -> Result<(HealthLease, bool), ExposureError> {
        validate(counters)?;
        if target < 0 || settled < 0 {
            return Err(ExposureError::Invalid);
        }
        bounded(async {
            let mut tx = self.pool.begin().await?;
            settings(&mut tx).await?;
            let row =
                sqlx::query("SELECT * FROM flow_observation_epochs WHERE epoch=$1 FOR UPDATE")
                    .bind(cut.lease.epoch)
                    .fetch_one(&mut *tx)
                    .await?;
            let now = now_ms(&mut tx).await?;
            live(&row, now)?;
            let confirmed: i64 = row.try_get("confirmed_ms")?;
            let pending: Option<i64> = row.try_get("pending_cut_ms")?;
            let old_target: Option<i64> = row.try_get("pending_target")?;
            if pending != Some(cut.cut_ms) && !(pending.is_none() && confirmed == cut.cut_ms) {
                return Err(ExposureError::Invalid);
            }
            if cut.cut_ms > now {
                return Err(ExposureError::Unavailable);
            }
            let (lost, failed) = deltas(&row, counters)?;
            flag(
                &mut tx,
                cut.lease.epoch,
                confirmed.saturating_sub(902_000),
                now,
                lost,
                failed,
                false,
            )
            .await?;
            let target = target.max(old_target.unwrap_or(0));
            let complete = settled >= target || pending.is_none();
            sqlx::query(
                r#"UPDATE flow_observation_epochs SET lease_until_ms=$2+30000,
 confirmed_ms=CASE WHEN $3 THEN $4 ELSE confirmed_ms END,
 pending_cut_ms=CASE WHEN $3 THEN NULL ELSE $4 END,
 pending_target=CASE WHEN $3 THEN NULL ELSE $5 END,
 dropped_seen=$6,failed_seen=$7 WHERE epoch=$1"#,
            )
            .bind(cut.lease.epoch)
            .bind(now)
            .bind(complete)
            .bind(cut.cut_ms)
            .bind(target)
            .bind(counters.dropped)
            .bind(counters.failed)
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
            Ok((
                HealthLease {
                    epoch: cut.lease.epoch,
                    clock_ms: now,
                    lease_until_ms: now + 30_000,
                },
                complete,
            ))
        })
        .await
    }
}
fn live(row: &sqlx::postgres::PgRow, now: i64) -> Result<(), ExposureError> {
    let confirmed: i64 = row.try_get("confirmed_ms")?;
    if row.try_get::<bool, _>("retired")?
        || row.try_get::<i64, _>("lease_until_ms")? <= now
        || confirmed < now.saturating_sub(120_000)
        || confirmed > now
    {
        Err(ExposureError::Expired)
    } else {
        Ok(())
    }
}
