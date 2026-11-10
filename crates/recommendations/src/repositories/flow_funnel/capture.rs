use super::{unavailable, PgFlowFunnel};
use crate::services::flow_funnel::FlowFunnelError;

impl PgFlowFunnel {
    pub(super) async fn capture_books(&self) -> Result<u64, FlowFunnelError> {
        let mut affected = 0;
        for _ in 0..16 {
            let mut tx = self.begin().await?;
            let subject: Option<String> = sqlx::query_scalar(
                "SELECT s.subject_id::text FROM rec_subjects s WHERE EXISTS(
                   SELECT 1 FROM flow_visit_receipts r WHERE r.subject_id=s.subject_id
                     AND r.book_id IS NOT NULL AND NOT r.funnel_processed
                     AND r.observed_at<=clock_timestamp()-interval '305 seconds')
                 ORDER BY s.subject_id LIMIT 1 FOR UPDATE OF s SKIP LOCKED",
            )
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?;
            let Some(subject) = subject else { break };
            sqlx::query(CAPTURE_SQL)
                .bind(&subject)
                .execute(&mut *tx)
                .await
                .map_err(unavailable)?;
            affected += sqlx::query(
                "UPDATE flow_visit_receipts r SET funnel_processed=true,
                   cohort_id=(
                     SELECT c.id FROM flow_book_cohorts c JOIN flow_visits v ON v.id=c.visit_id
                     WHERE c.subject_id=r.subject_id AND c.flow_epoch=r.flow_epoch AND c.book_id=r.book_id
                       AND r.observed_at>=v.started_at AND r.observed_at<=v.last_at
                       AND c.expires_at>clock_timestamp() AND c.finalized_at IS NULL
                       AND NOT r.continuation_unknown
                   )
                 WHERE r.event_id IN(SELECT event_id FROM flow_visit_receipts
                   WHERE subject_id=$1::uuid AND book_id IS NOT NULL AND NOT funnel_processed
                     AND observed_at<=clock_timestamp()-interval '305 seconds'
                   ORDER BY observed_at,event_id LIMIT 256)",
            )
            .bind(&subject)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?
            .rows_affected();
            tx.commit().await.map_err(unavailable)?;
        }
        Ok(affected)
    }
}

// A book entry is sealed only after every still-admissible preceding page has
// had its five-minute delivery grace. Disjoint visit intervals identify it;
// no mutable per-receipt visit FK or rewrite on late session merge is needed.
const CAPTURE_SQL: &str = r#"
WITH picked AS MATERIALIZED (
 SELECT * FROM flow_visit_receipts WHERE subject_id=$1::uuid
   AND book_id IS NOT NULL AND NOT funnel_processed
   AND observed_at<=clock_timestamp()-interval '305 seconds'
 ORDER BY observed_at,event_id LIMIT 256
), groups AS (
 SELECT DISTINCT v.id AS visit_id,p.subject_id,p.flow_epoch,p.book_id,
        v.started_at,v.last_at,v.acquisition_hint
 FROM picked p JOIN flow_visits v ON v.subject_id=p.subject_id AND v.flow_epoch=p.flow_epoch
   AND p.observed_at>=v.started_at AND p.observed_at<=v.last_at
 JOIN rec_subjects s ON s.subject_id=p.subject_id AND s.flow_epoch=p.flow_epoch AND s.enabled
 WHERE NOT p.continuation_unknown AND v.started_at>clock_timestamp()-interval '30 days'
), entries AS (
 SELECT g.*,first.event_id,first.observed_at,first.discovery_kind,
   EXISTS(SELECT 1 FROM flow_visit_receipts r WHERE r.subject_id=g.subject_id AND r.flow_epoch=g.flow_epoch
     AND r.book_id=g.book_id AND r.book_kind=0 AND r.observed_at>=g.started_at AND r.observed_at<=g.last_at) AS has_detail,
   EXISTS(SELECT 1 FROM flow_visit_receipts r WHERE r.subject_id=g.subject_id AND r.flow_epoch=g.flow_epoch
     AND r.book_id=g.book_id AND r.book_kind=1 AND r.observed_at>=g.started_at AND r.observed_at<=g.last_at) AS has_reader
 FROM groups g CROSS JOIN LATERAL (
   SELECT r.event_id,r.observed_at,r.discovery_kind FROM flow_visit_receipts r
   WHERE r.subject_id=g.subject_id AND r.flow_epoch=g.flow_epoch AND r.book_id=g.book_id
     AND r.observed_at>=g.started_at AND r.observed_at<=g.last_at AND NOT r.continuation_unknown
   ORDER BY r.observed_at,r.event_id LIMIT 1
 ) first
)
INSERT INTO flow_book_cohorts(id,subject_id,flow_epoch,visit_id,visit_started_at,book_id,
 first_entry_at,expires_at,acquisition_hint,discovery_kind,has_detail,has_reader)
SELECT event_id,subject_id,flow_epoch,visit_id,started_at,book_id,observed_at,
       started_at+interval '30 days',acquisition_hint,discovery_kind,has_detail,has_reader FROM entries
ON CONFLICT(subject_id,flow_epoch,visit_id,book_id) DO UPDATE SET
 has_detail=flow_book_cohorts.has_detail OR EXCLUDED.has_detail,
 has_reader=flow_book_cohorts.has_reader OR EXCLUDED.has_reader
WHERE flow_book_cohorts.finalized_at IS NULL AND flow_book_cohorts.expires_at>clock_timestamp()
"#;
