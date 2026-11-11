//! Bounded ITT reports and the experiment safety/retention tick.
//! Cross-domain capture is exclusively I's canonical C trigger contract.
use super::{admin::live_admin, PgSurprises, SurpriseError};
use crate::services::surprise_experiment::{assess, ArmCounts, Difference};
use sqlx::{Postgres, Row, Transaction};

#[derive(Debug)]
pub struct ExperimentReport {
    pub id: i64,
    pub state: String,
    pub decision: String,
    pub started_at: String,
    pub enrollment_ends_at: String,
    pub ends_at: String,
    pub measured_at: String,
    pub counts: [ArmCounts; 3],
    pub differences: Option<[Difference; 2]>,
}

async fn counts(
    tx: &mut Transaction<'_, Postgres>,
    run: i64,
) -> Result<[ArmCounts; 3], SurpriseError> {
    let rows = sqlx::query(
        "SELECT arm,enrolled,mature,meaningful,baseline_eligible,current_completed,
                returned_d1,returned_d7,disabled,prior_readers
         FROM surprise_experiment_totals WHERE run_id=$1 ORDER BY arm",
    )
    .bind(run)
    .fetch_all(&mut **tx)
    .await?;
    if rows.len() != 3 {
        return Err(SurpriseError::Unavailable);
    }
    let mut result = [ArmCounts::default(); 3];
    for (index, row) in rows.into_iter().enumerate() {
        let arm: &str = row.try_get("arm")?;
        if arm != ["control", "gentle", "standard"][index] {
            return Err(SurpriseError::Unavailable);
        }
        result[index] = ArmCounts {
            enrolled: row.try_get("enrolled")?,
            mature: row.try_get("mature")?,
            meaningful: row.try_get("meaningful")?,
            baseline_eligible: row.try_get("baseline_eligible")?,
            current_completed: row.try_get("current_completed")?,
            returned_d1: row.try_get("returned_d1")?,
            returned_d7: row.try_get("returned_d7")?,
            disabled: row.try_get("disabled")?,
            prior_readers: row.try_get("prior_readers")?,
        };
    }
    Ok(result)
}

impl PgSurprises {
    /// Read-only administrative report. Collection and cleanup never depend on
    /// someone opening this page. Closed trials retain only their frozen totals.
    pub async fn admin_reading_experiment(
        &self,
        digest: &[u8; 32],
    ) -> Result<Option<ExperimentReport>, SurpriseError> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ")
            .execute(&mut *tx)
            .await?;
        sqlx::query("SET LOCAL statement_timeout='750ms'")
            .execute(&mut *tx)
            .await?;
        live_admin(&mut tx, digest).await?;
        let row = sqlx::query(
            "SELECT e.id,e.state,e.decision,
               to_char(e.started_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS') AS started,
               to_char(e.enrollment_ends_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS') AS enrollment_end,
               to_char(e.ends_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI:SS') AS trial_end,
               COALESCE((SELECT to_char(min(t.measured_at) AT TIME ZONE 'UTC',
                  'YYYY-MM-DD HH24:MI:SS') FROM surprise_experiment_totals t
                  WHERE t.run_id=e.id),'') AS measured,
               e.faults_start=COALESCE(e.faults_end,surprise_capture_faults()) AS healthy
             FROM surprise_experiment_runs e ORDER BY e.id DESC LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await?;
        let Some(row) = row else {
            return Ok(None);
        };
        let id = row.try_get("id")?;
        let state: String = row.try_get("state")?;
        let observed = counts(&mut tx, id).await?;
        let assessment = assess(&observed, state == "completed", row.try_get("healthy")?);
        let report = ExperimentReport {
            id,
            differences: if state == "completed" {
                assessment.new_book_differences
            } else {
                None
            },
            decision: row.try_get("decision")?,
            state,
            started_at: row.try_get("started")?,
            enrollment_ends_at: row.try_get("enrollment_end")?,
            ends_at: row.try_get("trial_end")?,
            measured_at: row.try_get("measured")?,
            counts: observed,
        };
        tx.commit().await?;
        Ok(Some(report))
    }

    /// A single bounded tick, including when the feature is disabled. Returns
    /// false when an expired receipt backlog still needs the next tick.
    pub async fn maintain_experiment(&self) -> Result<bool, SurpriseError> {
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
            return Ok(true);
        }
        let row = sqlx::query(
            "SELECT id,ends_at<=transaction_timestamp() AS finished,
                    faults_start=surprise_capture_faults() AS healthy,
                    last_checked_at<transaction_timestamp()-interval '2 minutes' AS monitor_gap
             FROM surprise_experiment_runs WHERE state='running' ORDER BY id LIMIT 1",
        )
        .fetch_optional(&mut *tx)
        .await?;
        if let Some(row) = row {
            let id: i64 = row.try_get("id")?;
            let finished: bool = row.try_get("finished")?;
            let healthy: bool = row.try_get("healthy")?;
            let monitor_gap: bool = row.try_get("monitor_gap")?;
            sqlx::query("SELECT surprise_refresh_totals($1)")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            let assessment = assess(&counts(&mut tx, id).await?, finished, healthy);
            if assessment.stop || monitor_gap {
                // Same lock order as administrative policy changes. C's shared
                // run lock makes all already accepted captures finish first.
                let current: Option<i64> = sqlx::query_scalar(
                    "SELECT experiment_run_id FROM surprise_runtime_policy
                     WHERE singleton AND experiment_enabled FOR UPDATE",
                )
                .fetch_optional(&mut *tx)
                .await?
                .flatten();
                if current == Some(id) {
                    let sealed: Option<i64> = sqlx::query_scalar(
                        "UPDATE surprise_experiment_runs SET
                           state=CASE WHEN $2 THEN 'completed' ELSE 'stopped' END,
                           closed_at=clock_timestamp(),faults_end=surprise_capture_faults(),
                           last_checked_at=clock_timestamp()
                         WHERE id=$1 AND state='running' RETURNING id",
                    )
                    .bind(id)
                    .bind(finished && healthy && !monitor_gap)
                    .fetch_optional(&mut *tx)
                    .await?;
                    if sealed.is_some() {
                        // Freeze totals only AFTER acquiring the exclusive run
                        // fence, not from the preliminary monitoring snapshot.
                        sqlx::query("SELECT surprise_refresh_totals($1)")
                            .bind(id)
                            .execute(&mut *tx)
                            .await?;
                        let final_health: bool = sqlx::query_scalar(
                            "SELECT faults_start=surprise_capture_faults()
                             FROM surprise_experiment_runs WHERE id=$1",
                        )
                        .bind(id)
                        .fetch_one(&mut *tx)
                        .await?;
                        let final_assessment =
                            assess(&counts(&mut tx, id).await?, finished, final_health);
                        let code = if monitor_gap {
                            "monitor_gap"
                        } else {
                            final_assessment.code
                        };
                        sqlx::query(
                            "UPDATE surprise_experiment_runs SET decision=$2,
                               faults_end=surprise_capture_faults() WHERE id=$1",
                        )
                        .bind(id)
                        .bind(code)
                        .execute(&mut *tx)
                        .await?;
                        sqlx::query(
                            "UPDATE surprise_runtime_policy SET enabled=false,
                               experiment_enabled=false,revision=revision+1,updated_at=now()
                             WHERE singleton AND experiment_run_id=$1",
                        )
                        .bind(id)
                        .execute(&mut *tx)
                        .await?;
                    }
                }
            } else {
                sqlx::query(
                    "UPDATE surprise_experiment_runs SET last_checked_at=clock_timestamp(),
                       decision=$2 WHERE id=$1 AND state='running'",
                )
                .bind(id)
                .bind(assessment.code)
                .execute(&mut *tx)
                .await?;
            }
        }
        sqlx::query("SELECT surprise_purge_experiment_batch()")
            .execute(&mut *tx)
            .await?;
        let complete: bool = sqlx::query_scalar(
            "SELECT NOT EXISTS(SELECT 1 FROM surprise_daily_decisions
                WHERE utc_day<((clock_timestamp() AT TIME ZONE 'UTC')::date-DATE '1970-01-01')::bigint-30)
             AND NOT EXISTS(SELECT 1 FROM surprise_experiment_participants p
                JOIN surprise_experiment_runs e ON e.id=p.run_id
                WHERE p.enrolled_at<clock_timestamp()-interval '30 days' AND e.state<>'running')",
        )
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(complete)
    }
}
