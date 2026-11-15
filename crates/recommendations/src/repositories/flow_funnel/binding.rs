use super::{
    events::{ReadEvidence, Scope},
    unavailable,
};
use crate::services::flow_funnel::FlowFunnelError;
use sqlx::{Postgres, Transaction};

pub(super) async fn waiting_for_books(
    tx: &mut Transaction<'_, Postgres>,
    scope: &Scope,
    start: &ReadEvidence,
) -> Result<bool, FlowFunnelError> {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM flow_visit_receipts WHERE subject_id=$1::uuid AND flow_epoch=$2::uuid
           AND book_id=$3 AND NOT funnel_processed AND observed_at<=$4::timestamptz
           AND observed_at>=$4::timestamptz-interval '7 days')",
    )
    .bind(&scope.subject).bind(&scope.epoch).bind(start.book).bind(&start.at)
    .fetch_one(&mut **tx).await.map_err(unavailable)
}

pub(super) async fn apply_start(
    tx: &mut Transaction<'_, Postgres>,
    scope: &Scope,
    start: &ReadEvidence,
) -> Result<Option<String>, FlowFunnelError> {
    if !super::events::lock_proof(tx, scope, start).await? {
        return Ok(None);
    }
    let cohort: Option<String> = sqlx::query_scalar(
        "SELECT c.id::text FROM flow_book_cohorts c
         WHERE c.subject_id=$1::uuid AND c.flow_epoch=$2::uuid AND c.book_id=$3
           AND c.first_entry_at<=$4::timestamptz AND c.first_entry_at>=$4::timestamptz-interval '7 days'
           AND c.expires_at>clock_timestamp() AND c.expires_at>$4::timestamptz AND c.finalized_at IS NULL
           AND $4::timestamptz>=$5::timestamptz
           AND EXISTS(SELECT 1 FROM reader_flow_verified_read_receipts r WHERE r.event_id=$6::uuid
                      AND r.subject_id=c.subject_id AND r.flow_epoch=c.flow_epoch AND r.occurred_at=$4::timestamptz)
         ORDER BY c.first_entry_at DESC,c.id LIMIT 1 FOR UPDATE",
    )
    .bind(&scope.subject).bind(&scope.epoch).bind(start.book).bind(&start.at).bind(&scope.enabled_at)
    .bind(&start.id)
    .fetch_optional(&mut **tx).await.map_err(unavailable)?;
    let Some(cohort) = cohort else {
        return Ok(None);
    };
    sqlx::query(
        "UPDATE flow_book_cohorts SET started=true,new_reader=new_reader OR $4,
           first_start_event_id=CASE WHEN first_start_at IS NULL OR
             (first_start_at,first_start_event_id)>($3::timestamptz,$2::uuid)
             THEN $2::uuid ELSE first_start_event_id END,
           first_start_at=LEAST(COALESCE(first_start_at,$3::timestamptz),$3::timestamptz)
         WHERE id=$1::uuid",
    )
    .bind(&cohort)
    .bind(&start.id)
    .bind(&start.at)
    .bind(start.first)
    .execute(&mut **tx)
    .await
    .map_err(unavailable)?;
    super::reconciliation::reconcile(tx, &scope.subject, Some(start.book)).await?;
    Ok(Some(cohort))
}
