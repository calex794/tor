//! Only claims with no provider reservation or durable correlation may
//! return to pending. A committed/unknown send attempt is never replayed here.
use super::PgMailQueue;
use crate::domain::{
    dispatch::{DeferOutcome, DeferReason, MAX_PRE_SUBMISSION_CLAIMS},
    MailClaim,
};

impl PgMailQueue {
    pub async fn defer_unsubmitted(
        &self,
        claim: MailClaim,
        reason: DeferReason,
    ) -> Result<DeferOutcome, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // Same lock order as quota/feedback (guard, then queue row).
        sqlx::query_scalar::<_, bool>(
            "SELECT singleton FROM mail_send_budget_guard WHERE singleton FOR UPDATE",
        )
        .fetch_one(&mut *tx)
        .await?;
        let exhausted = claim.generation >= MAX_PRE_SUBMISSION_CLAIMS;
        let status = if exhausted { "failed" } else { "pending" };
        let affected = sqlx::query(
            "UPDATE mail_outbox m SET status=$6,lease_until=NULL,
             not_before=clock_timestamp()+($7::bigint*interval '1 second'),
             defer_reason=$8,
             finished_at=CASE WHEN $6='failed' THEN clock_timestamp() ELSE NULL END
             WHERE m.id=$1 AND m.generation=$2 AND m.purpose=$3 AND m.event_id=$4
               AND m.recipient_user_id=$5 AND m.status='leased'
               AND m.lease_until>clock_timestamp()
               AND NOT EXISTS (SELECT 1 FROM mail_send_reservations r
                 WHERE r.outbox_id=m.id AND r.generation=m.generation)
               AND NOT EXISTS (SELECT 1 FROM mail_feedback_attempts f
                 WHERE f.outbox_id=m.id AND f.generation=m.generation)",
        )
        .bind(claim.id)
        .bind(claim.generation)
        .bind(claim.intent.purpose.as_str())
        .bind(claim.intent.event_id.get())
        .bind(claim.intent.recipient.get())
        .bind(status)
        .bind(reason.delay_seconds(claim.generation))
        .bind(reason.as_str())
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(match (affected, exhausted) {
            (1, false) => DeferOutcome::Deferred,
            (1, true) => DeferOutcome::Exhausted,
            _ => DeferOutcome::Refused,
        })
    }
}
