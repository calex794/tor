//! Retirement housekeeping freezes observations already stored by the old model.
use super::{maintenance::FINALIZE_SQL, unavailable, PgFlowFunnel, CONSUMER};
use crate::services::flow_funnel::FlowFunnelError;

impl PgFlowFunnel {
    /// Preserve TTL without accepting new C facts, reconciling first reads, or
    /// resolving returns from activity arriving after collection retirement.
    pub async fn maintain_retired(&self) -> Result<u64, FlowFunnelError> {
        self.initialize(false).await?;
        let mut affected = 0;
        for _ in 0..16 {
            let mut tx = self.begin().await?;
            let subject: Option<String> = sqlx::query_scalar(
                "SELECT s.subject_id::text FROM rec_subjects s
                 WHERE EXISTS(SELECT 1 FROM flow_book_cohorts c
                   WHERE c.subject_id=s.subject_id AND c.expires_at<=clock_timestamp())
                 ORDER BY s.subject_id LIMIT 1 FOR UPDATE OF s SKIP LOCKED",
            )
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?;
            let Some(subject) = subject else { break };
            // This shared SQL only uses persisted flags. At raw expiry an
            // unresolved D7 becomes unknown; it is never inferred as a return.
            sqlx::query(FINALIZE_SQL)
                .bind(&subject)
                .execute(&mut *tx)
                .await
                .map_err(unavailable)?;
            affected += sqlx::query(
                "DELETE FROM flow_book_cohorts WHERE id IN(
                   SELECT id FROM flow_book_cohorts WHERE subject_id=$1::uuid
                     AND finalized_at IS NOT NULL AND expires_at<=clock_timestamp()
                   ORDER BY expires_at,id LIMIT 256)",
            )
            .bind(subject)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?
            .rows_affected();
            tx.commit().await.map_err(unavailable)?;
        }
        // Only receipt existence/time is used for removing orphan or expired
        // delivery markers. No canonical payload or activity enriches a cohort.
        affected += self.prune_expired().await?;
        let mut tx = self.begin().await?;
        let backlog: bool = sqlx::query_scalar(RETIRED_BACKLOG_SQL)
            .bind(CONSUMER)
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query(
            "UPDATE flow_book_worker_health SET
               last_collecting=false,collecting_since=NULL,last_tick=clock_timestamp(),
               backlog=$1,last_success=CASE WHEN $1 THEN last_success ELSE clock_timestamp() END
             WHERE singleton",
        )
        .bind(backlog)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(affected)
    }
}

const RETIRED_BACKLOG_SQL: &str = r#"
SELECT EXISTS(SELECT 1 FROM flow_book_cohorts WHERE expires_at<=clock_timestamp())
 OR EXISTS(SELECT 1 FROM flow_book_action_receipts WHERE occurred_at<clock_timestamp()-interval '30 days')
 OR EXISTS(SELECT 1 FROM flow_book_daily WHERE cohort_day<(clock_timestamp() AT TIME ZONE 'UTC')::date-179)
 OR EXISTS(SELECT 1 FROM event_deliveries d WHERE d.consumer=$1 AND NOT EXISTS(
       SELECT 1 FROM reader_flow_verified_read_receipts r WHERE r.event_id=d.event_id
         AND r.occurred_at>clock_timestamp()-interval '30 days'))
"#;
