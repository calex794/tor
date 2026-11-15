//! Canonical occurrence time can precede commit. Reconcile only retained raw
//! cohorts; the anonymous archive is the evidence known at finalization.
use super::unavailable;
use crate::services::flow_funnel::FlowFunnelError;
use sqlx::{Postgres, Row, Transaction};

pub(super) async fn reconcile(
    tx: &mut Transaction<'_, Postgres>,
    subject: &str,
    book: Option<i64>,
) -> Result<bool, FlowFunnelError> {
    // Caller already holds the subject. A late start can relocate an anchor
    // even when it acquired its delivery before this reconciliation ran.
    let rows = sqlx::query(TARGETS_SQL)
        .bind(subject)
        .bind(book)
        .fetch_all(&mut **tx)
        .await
        .map_err(unavailable)?;
    let drained = rows.len() < 64;
    for row in rows {
        let book: i64 = row.try_get("book_id").map_err(unavailable)?;
        sqlx::query(
            "UPDATE flow_book_cohorts c SET new_reader=EXISTS(
               SELECT 1 FROM reader_flow_verified_read_events v
               WHERE v.event_id=c.first_start_event_id AND v.kind=0 AND v.first_for_book)
             WHERE c.subject_id=$1::uuid AND c.book_id=$2 AND c.started AND c.finalized_at IS NULL",
        )
        .bind(subject)
        .bind(book)
        .execute(&mut **tx)
        .await
        .map_err(unavailable)?;
        let target: Option<String> = row.try_get("target_id").map_err(unavailable)?;
        let target_at: Option<String> = row.try_get("target_at").map_err(unavailable)?;
        let old: Option<String> = row.try_get("old_id").map_err(unavailable)?;
        let old_at: Option<String> = row.try_get("old_at").map_err(unavailable)?;
        if (target.as_ref(), target_at.as_ref()) == (old.as_ref(), old_at.as_ref()) {
            continue;
        }
        sqlx::query(
            "DELETE FROM flow_book_return_anchors WHERE subject_id=$1::uuid AND book_id=$2",
        )
        .bind(subject)
        .bind(book)
        .execute(&mut **tx)
        .await
        .map_err(unavailable)?;
        sqlx::query(
            "UPDATE flow_book_cohorts SET d7_state=0
             WHERE subject_id=$1::uuid AND book_id=$2 AND d7_state>0 AND finalized_at IS NULL",
        )
        .bind(subject)
        .bind(book)
        .execute(&mut **tx)
        .await
        .map_err(unavailable)?;
        if let (Some(target), Some(at)) = (target, target_at) {
            sqlx::query(
                "INSERT INTO flow_book_return_anchors(subject_id,flow_epoch,book_id,cohort_id,started_at,expires_at)
                 SELECT subject_id,flow_epoch,book_id,id,$2::timestamptz,expires_at
                 FROM flow_book_cohorts WHERE id=$1::uuid AND finalized_at IS NULL",
            ).bind(&target).bind(at).execute(&mut **tx).await.map_err(unavailable)?;
            sqlx::query("UPDATE flow_book_cohorts SET d7_state=1 WHERE id=$1::uuid")
                .bind(target)
                .execute(&mut **tx)
                .await
                .map_err(unavailable)?;
        }
    }
    Ok(drained)
}

const TARGETS_SQL: &str = r#"
WITH books AS (
 SELECT DISTINCT book_id FROM flow_book_cohorts
 WHERE subject_id=$1::uuid AND started AND finalized_at IS NULL
   AND ($2::bigint IS NULL OR book_id=$2)
), targets AS (
 SELECT b.book_id,c.id AS target_id,CASE WHEN c.id IS NOT NULL THEN first.occurred_at END AS target_at,
        a.cohort_id AS old_id,a.started_at AS old_at
 FROM books b JOIN rec_subjects s ON s.subject_id=$1::uuid
 LEFT JOIN LATERAL (
   SELECT v.event_id,v.occurred_at FROM reader_flow_verified_read_events v
   WHERE v.user_id=s.user_id AND v.book_id=b.book_id AND v.kind=0 AND v.occurred_at>=s.flow_enabled_at
   ORDER BY v.occurred_at,v.event_id LIMIT 1
 ) first ON true
 LEFT JOIN flow_book_cohorts c ON c.subject_id=s.subject_id AND c.flow_epoch=s.flow_epoch
   AND c.book_id=b.book_id AND c.first_start_event_id=first.event_id AND c.finalized_at IS NULL
 LEFT JOIN flow_book_return_anchors a ON a.subject_id=s.subject_id AND a.flow_epoch=s.flow_epoch AND a.book_id=b.book_id
)
SELECT t.book_id,t.target_id::text,t.target_at::text,t.old_id::text,t.old_at::text
FROM targets t
WHERE (t.target_id,t.target_at) IS DISTINCT FROM (t.old_id,t.old_at)
 OR EXISTS(SELECT 1 FROM flow_book_cohorts c WHERE c.subject_id=$1::uuid AND c.book_id=t.book_id
   AND c.started AND c.finalized_at IS NULL AND c.new_reader IS DISTINCT FROM
   EXISTS(SELECT 1 FROM reader_flow_verified_read_events v
     WHERE v.event_id=c.first_start_event_id AND v.kind=0 AND v.first_for_book))
ORDER BY t.book_id LIMIT 64
"#;
