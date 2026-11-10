use super::{unavailable, PgFlowVisits};
use crate::services::visits::FlowVisitError;

impl PgFlowVisits {
    /// Finalization and expiry run even when capture is disabled. Each subject
    /// has its own transaction, so erasure and workers share one lock order.
    /// A visit is immutable after 35 minutes: accepted queue delay <=5 minutes
    /// cannot bridge the strictly-less-than-30-minute idle boundary afterwards.
    pub async fn maintain(&self) -> Result<u64, FlowVisitError> {
        let mut affected = 0_u64;
        for _ in 0..16 {
            let mut tx = self.pool.begin().await.map_err(unavailable)?;
            sqlx::query("SET LOCAL statement_timeout='750ms'")
                .execute(&mut *tx)
                .await
                .map_err(unavailable)?;
            let subject: Option<String> = sqlx::query_scalar(
                "SELECT s.subject_id::text FROM rec_subjects s
                 WHERE EXISTS(SELECT 1 FROM flow_visits v WHERE v.subject_id=s.subject_id
                   AND ((v.finalized_at IS NULL AND v.last_at<clock_timestamp()-interval '35 minutes')
                     OR v.started_at<clock_timestamp()-interval '30 days'))
                 ORDER BY s.subject_id LIMIT 1 FOR UPDATE OF s SKIP LOCKED",
            )
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?;
            let Some(subject) = subject else {
                break;
            };
            // For an activity window lasting over thirty days, erase its old
            // origin but remember recent continuity for idle+queue-delay only.
            sqlx::query(
                "INSERT INTO flow_visit_continuations(subject_id,flow_epoch,last_at)
                 SELECT subject_id,flow_epoch,max(last_at) FROM flow_visits
                 WHERE subject_id=$1::uuid AND started_at<clock_timestamp()-interval '30 days'
                   AND last_at>clock_timestamp()-interval '35 minutes'
                 GROUP BY subject_id,flow_epoch
                 ON CONFLICT(subject_id,flow_epoch) DO UPDATE
                   SET last_at=GREATEST(flow_visit_continuations.last_at,EXCLUDED.last_at)",
            )
            .bind(&subject)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
            let changed = sqlx::query(
                "WITH due AS MATERIALIZED (
                   SELECT id FROM flow_visits WHERE subject_id=$1::uuid AND finalized_at IS NULL
                     AND (last_at<clock_timestamp()-interval '35 minutes'
                       OR started_at<clock_timestamp()-interval '30 days')
                   ORDER BY started_at,id LIMIT 500 FOR UPDATE
                 ), finalized AS (
                   UPDATE flow_visits v SET finalized_at=GREATEST(clock_timestamp(),last_at)
                   FROM due WHERE v.id=due.id
                   RETURNING v.started_at,v.last_at,v.acquisition_hint,v.landing_surface,
                             v.last_surface,v.pageviews
                 )
                 INSERT INTO flow_visit_daily
                   (cohort_day,acquisition_hint,landing_surface,last_surface,visits,pageviews,observed_seconds)
                 SELECT (started_at AT TIME ZONE 'UTC')::date,acquisition_hint,landing_surface,
                        last_surface,count(*),sum(pageviews)::bigint,
                        sum(floor(extract(epoch FROM (last_at-started_at)))::bigint)::bigint
                 FROM finalized
                 GROUP BY 1,2,3,4 ORDER BY 1,2,3,4
                 ON CONFLICT(cohort_day,acquisition_hint,landing_surface,last_surface)
                 DO UPDATE SET visits=flow_visit_daily.visits+EXCLUDED.visits,
                   pageviews=flow_visit_daily.pageviews+EXCLUDED.pageviews,
                   observed_seconds=flow_visit_daily.observed_seconds+EXCLUDED.observed_seconds",
            )
            .bind(&subject)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
            affected += changed.rows_affected();
            // Only finalized rows can become irreversibly anonymous. Receipt
            // expiry does not make an old event acceptable: observe rejects >5m.
            for statement in [
                "DELETE FROM flow_visits WHERE id IN(SELECT id FROM flow_visits
                   WHERE subject_id=$1::uuid AND finalized_at IS NOT NULL
                     AND started_at<clock_timestamp()-interval '30 days'
                   ORDER BY started_at,id LIMIT 500)",
                "DELETE FROM flow_visit_receipts WHERE event_id IN(SELECT event_id
                   FROM flow_visit_receipts WHERE subject_id=$1::uuid
                     AND observed_at<clock_timestamp()-interval '30 days'
                   ORDER BY observed_at,event_id LIMIT 1000)",
            ] {
                affected += sqlx::query(statement)
                    .bind(&subject)
                    .execute(&mut *tx)
                    .await
                    .map_err(unavailable)?
                    .rows_affected();
            }
            tx.commit().await.map_err(unavailable)?;
        }
        // Some subjects have only expiring receipts/day rows left. These
        // bounded deletions cannot race a valid new observation into old data.
        for statement in [
            "DELETE FROM flow_visit_continuations WHERE (subject_id,flow_epoch) IN(
               SELECT subject_id,flow_epoch FROM flow_visit_continuations
               WHERE last_at<clock_timestamp()-interval '35 minutes'
               ORDER BY last_at,subject_id,flow_epoch LIMIT 1000)
               AND last_at<clock_timestamp()-interval '35 minutes'",
            "DELETE FROM flow_visit_receipts WHERE event_id IN(SELECT event_id FROM flow_visit_receipts
               WHERE observed_at<clock_timestamp()-interval '30 days'
               ORDER BY observed_at,event_id LIMIT 1000)",
            "DELETE FROM flow_visit_account_days WHERE (subject_id,flow_epoch,bucket_date) IN(
               SELECT subject_id,flow_epoch,bucket_date FROM flow_visit_account_days
               WHERE bucket_date<(clock_timestamp() AT TIME ZONE 'UTC')::date-29
               ORDER BY bucket_date,subject_id,flow_epoch LIMIT 1000)",
            "DELETE FROM flow_visit_daily WHERE (cohort_day,acquisition_hint,landing_surface,last_surface) IN(
               SELECT cohort_day,acquisition_hint,landing_surface,last_surface FROM flow_visit_daily
               WHERE cohort_day<(clock_timestamp() AT TIME ZONE 'UTC')::date-179
               ORDER BY cohort_day,acquisition_hint,landing_surface,last_surface LIMIT 1000)",
        ] {
            affected += sqlx::query(statement)
                .execute(&self.pool)
                .await
                .map_err(unavailable)?
                .rows_affected();
        }
        Ok(affected)
    }
}
