//! Independent fail-closed recovery: a broken/oversized aggregate must not
//! prevent stopping an expired monitor or purging old bounded receipts.
use super::{PgSurprises, SurpriseError};

impl PgSurprises {
    pub async fn recover_expired_experiment(&self) -> Result<(), SurpriseError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET LOCAL statement_timeout='750ms'")
            .execute(&mut *tx)
            .await?;
        sqlx::query("SET LOCAL lock_timeout='25ms'")
            .execute(&mut *tx)
            .await?;
        let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(82483404)")
            .fetch_one(&mut *tx)
            .await?;
        if !acquired {
            return Ok(());
        }
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT experiment_run_id FROM surprise_runtime_policy
             WHERE singleton AND experiment_enabled FOR UPDATE",
        )
        .fetch_optional(&mut *tx)
        .await?
        .flatten();
        if let Some(id) = current {
            let sealed: Option<i64> = sqlx::query_scalar(
                "UPDATE surprise_experiment_runs SET state='stopped',decision='monitor_gap',
                   closed_at=clock_timestamp(),faults_end=surprise_capture_faults()
                 WHERE id=$1 AND state='running'
                   AND last_checked_at<clock_timestamp()-interval '2 minutes' RETURNING id",
            )
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
            if sealed.is_some() {
                // The run is already sealed, so the policy trigger must not
                // attempt the unavailable aggregate again. This is NOT a
                // completed trial; retain only the last measured snapshot.
                sqlx::query(
                    "UPDATE surprise_runtime_policy SET enabled=false,experiment_enabled=false,
                       revision=revision+1,updated_at=now()
                     WHERE singleton AND experiment_run_id=$1",
                )
                .bind(id)
                .execute(&mut *tx)
                .await?;
            }
        }
        sqlx::query("SELECT surprise_purge_experiment_batch()")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
