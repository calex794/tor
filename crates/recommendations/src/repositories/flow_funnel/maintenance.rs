use super::{unavailable, PgFlowFunnel, CONSUMER};
use crate::services::flow_funnel::FlowFunnelError;
use sqlx::{Postgres, Transaction};

impl PgFlowFunnel {
    /// Resolve only witnessed positive returns. Missing target-day evidence is
    /// unknown, including a cohort whose visit-origin retention ends early.
    /// Subject -> ordered daily keys matches the consent erasure lock order.
    pub(super) async fn maintain(&self) -> Result<u64, FlowFunnelError> {
        let mut affected = 0;
        for _ in 0..16 {
            let mut tx = self.begin().await?;
            let selection = format!(
                "SELECT s.subject_id::text FROM rec_subjects s WHERE {SUBJECT_WORK}
                ORDER BY s.subject_id LIMIT 1 FOR UPDATE OF s SKIP LOCKED"
            );
            let subject: Option<String> = sqlx::query_scalar(&selection)
                .fetch_optional(&mut *tx)
                .await
                .map_err(unavailable)?;
            let Some(subject) = subject else { break };
            if !super::reconciliation::reconcile(&mut tx, &subject, None).await? {
                tx.commit().await.map_err(unavailable)?;
                continue;
            }
            affected += sqlx::query(RETURNS_SQL)
                .bind(&subject)
                .execute(&mut *tx)
                .await
                .map_err(unavailable)?
                .rows_affected();
            sqlx::query(FINALIZE_SQL)
                .bind(&subject)
                .execute(&mut *tx)
                .await
                .map_err(unavailable)?;
            affected += sqlx::query(
                "DELETE FROM flow_book_cohorts WHERE subject_id=$1::uuid
                   AND finalized_at IS NOT NULL AND expires_at<=clock_timestamp()",
            )
            .bind(subject)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?
            .rows_affected();
            tx.commit().await.map_err(unavailable)?;
        }
        let mut tx = self.begin().await?;
        affected += sqlx::query(
            "DELETE FROM event_deliveries d WHERE d.consumer=$1 AND d.event_id IN(
               SELECT old.event_id FROM event_deliveries old WHERE old.consumer=$1
                 AND NOT EXISTS(SELECT 1 FROM reader_flow_verified_read_receipts r
                   WHERE r.event_id=old.event_id AND r.occurred_at>clock_timestamp()-interval '30 days')
               ORDER BY old.event_id LIMIT 1000)",
        ).bind(CONSUMER).execute(&mut *tx).await.map_err(unavailable)?.rows_affected();
        affected += sqlx::query(
            "DELETE FROM flow_book_action_receipts WHERE event_id IN(
               SELECT event_id FROM flow_book_action_receipts
               WHERE occurred_at<clock_timestamp()-interval '30 days'
               ORDER BY occurred_at,event_id LIMIT 1000)",
        )
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?
        .rows_affected();
        affected += sqlx::query(
            "DELETE FROM flow_book_daily WHERE (cohort_day,book_id,acquisition_hint,discovery_kind) IN(
               SELECT cohort_day,book_id,acquisition_hint,discovery_kind FROM flow_book_daily
               WHERE cohort_day<(clock_timestamp() AT TIME ZONE 'UTC')::date-179
               ORDER BY cohort_day,book_id,acquisition_hint,discovery_kind LIMIT 1000)",
        )
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?
        .rows_affected();
        tx.commit().await.map_err(unavailable)?;
        Ok(affected)
    }
}

pub(super) async fn pending(tx: &mut Transaction<'_, Postgres>) -> Result<bool, FlowFunnelError> {
    let sql = format!("SELECT EXISTS(SELECT 1 FROM rec_subjects s WHERE {SUBJECT_WORK})");
    sqlx::query_scalar(&sql)
        .fetch_one(&mut **tx)
        .await
        .map_err(unavailable)
}

const SUBJECT_WORK: &str = r#"
EXISTS(SELECT 1 FROM flow_book_cohorts c
 WHERE c.subject_id=s.subject_id AND (
   c.expires_at<=clock_timestamp()
   OR (c.started AND c.finalized_at IS NULL AND c.new_reader IS DISTINCT FROM
       EXISTS(SELECT 1 FROM reader_flow_verified_read_events v
         WHERE v.event_id=c.first_start_event_id AND v.kind=0 AND v.first_for_book))
   OR (c.d7_state>0 AND c.finalized_at IS NULL AND NOT EXISTS(
       SELECT 1 FROM flow_book_return_anchors a JOIN reader_flow_verified_read_events v
       ON v.event_id=c.first_start_event_id AND v.occurred_at=a.started_at
       WHERE a.cohort_id=c.id AND v.event_id=(
         SELECT first.event_id FROM reader_flow_verified_read_events first
         WHERE first.user_id=s.user_id AND first.book_id=c.book_id AND first.kind=0
           AND first.occurred_at>=s.flow_enabled_at
         ORDER BY first.occurred_at,first.event_id LIMIT 1)))
   OR (c.d7_state=1 AND EXISTS(SELECT 1 FROM flow_book_return_anchors a WHERE a.cohort_id=c.id
       AND ((a.started_at AT TIME ZONE 'UTC')::date+8)::timestamp AT TIME ZONE 'UTC'<=clock_timestamp()))
   OR (c.d7_state IN(1,3) AND EXISTS(
       SELECT 1 FROM flow_book_return_anchors a JOIN reader_flow_verified_activity_days d
       ON d.subject_id=a.subject_id AND d.flow_epoch=a.flow_epoch AND d.book_id=a.book_id
         AND d.activity_day=(a.started_at AT TIME ZONE 'UTC')::date+7
       WHERE a.cohort_id=c.id AND d.first_seen_at<c.expires_at))))
"#;

const RETURNS_SQL: &str = r#"
WITH states AS (
 SELECT c.id,
   CASE WHEN EXISTS(SELECT 1 FROM reader_flow_verified_activity_days d
     WHERE d.subject_id=a.subject_id AND d.flow_epoch=a.flow_epoch AND d.book_id=a.book_id
       AND d.activity_day=(a.started_at AT TIME ZONE 'UTC')::date+7
       AND d.first_seen_at<c.expires_at) THEN 2
     WHEN c.expires_at<=clock_timestamp()
       OR ((a.started_at AT TIME ZONE 'UTC')::date+8)::timestamp AT TIME ZONE 'UTC'<=clock_timestamp()
     THEN 3 ELSE 1 END AS next_state
 FROM flow_book_cohorts c JOIN flow_book_return_anchors a ON a.cohort_id=c.id
 WHERE c.subject_id=$1::uuid AND c.d7_state IN(1,3) AND c.finalized_at IS NULL
)
UPDATE flow_book_cohorts c SET d7_state=s.next_state FROM states s
WHERE c.id=s.id AND c.d7_state<>s.next_state
"#;

// Finalization and deletion share a transaction. A retry either sees raw
// inputs, or the committed anonymous bucket, never both. The stored marker also
// lets the erasure helper retract a retained finalized input during recovery.
const FINALIZE_SQL: &str = r#"
WITH finalized AS (
 UPDATE flow_book_cohorts SET finalized_at=clock_timestamp(),
   d7_state=CASE WHEN d7_state=1 THEN 3 ELSE d7_state END
 WHERE id IN(SELECT id FROM flow_book_cohorts WHERE subject_id=$1::uuid
   AND expires_at<=clock_timestamp() AND finalized_at IS NULL
   ORDER BY expires_at,id LIMIT 256)
 RETURNING *
)
INSERT INTO flow_book_daily(cohort_day,book_id,acquisition_hint,discovery_kind,
 cohorts,detail_opens,reader_opens,starts,completions,saves,follows,new_readers,
 d7_anchors,d7_returned,d7_unknown)
SELECT (first_entry_at AT TIME ZONE 'UTC')::date,book_id,acquisition_hint,discovery_kind,
 count(*),count(*) FILTER(WHERE has_detail),count(*) FILTER(WHERE has_reader),
 count(*) FILTER(WHERE started),count(*) FILTER(WHERE completed),
 count(*) FILTER(WHERE saved),count(*) FILTER(WHERE followed),count(*) FILTER(WHERE new_reader),
 count(*) FILTER(WHERE d7_state>0),count(*) FILTER(WHERE d7_state=2),count(*) FILTER(WHERE d7_state=3)
FROM finalized
WHERE first_entry_at>=((clock_timestamp() AT TIME ZONE 'UTC')::date-179)::timestamp AT TIME ZONE 'UTC'
GROUP BY 1,2,3,4 ORDER BY 1,2,3,4
ON CONFLICT(cohort_day,book_id,acquisition_hint,discovery_kind) DO UPDATE SET
 cohorts=flow_book_daily.cohorts+EXCLUDED.cohorts,
 detail_opens=flow_book_daily.detail_opens+EXCLUDED.detail_opens,
 reader_opens=flow_book_daily.reader_opens+EXCLUDED.reader_opens,
 starts=flow_book_daily.starts+EXCLUDED.starts,
 completions=flow_book_daily.completions+EXCLUDED.completions,
 saves=flow_book_daily.saves+EXCLUDED.saves,follows=flow_book_daily.follows+EXCLUDED.follows,
 new_readers=flow_book_daily.new_readers+EXCLUDED.new_readers,
 d7_anchors=flow_book_daily.d7_anchors+EXCLUDED.d7_anchors,
 d7_returned=flow_book_daily.d7_returned+EXCLUDED.d7_returned,
 d7_unknown=flow_book_daily.d7_unknown+EXCLUDED.d7_unknown
"#;
