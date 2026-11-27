//! Durable, generation-fenced defer. Never retry a charged SMTP attempt.
use super::PgMailQueue;
use crate::domain::{
    dispatch::{DeferDecision, DeferReason, MAX_CLAIMS},
    MailClaim,
};

impl PgMailQueue {
    /// Serialize with quota reservation and correlation preparation. A failed
    /// or ambiguous transaction does not authorize either retry or SMTP.
    pub async fn defer_unreserved(
        &self,
        claim: MailClaim,
        reason: DeferReason,
    ) -> Result<DeferDecision, sqlx::Error> {
        use DeferDecision::*;
        let mut tx = self.pool.begin().await?;
        sqlx::query_scalar::<_, bool>(
            "SELECT singleton FROM mail_send_budget_guard WHERE singleton FOR UPDATE",
        )
        .fetch_one(&mut *tx)
        .await?;
        let active: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM mail_outbox WHERE id=$1 AND generation=$2 \
             AND purpose=$3 AND event_id=$4 AND recipient_user_id=$5 \
             AND status='leased' AND lease_until>clock_timestamp() FOR UPDATE",
        )
        .bind(claim.id)
        .bind(claim.generation)
        .bind(claim.intent.purpose.as_str())
        .bind(claim.intent.event_id.get())
        .bind(claim.intent.recipient.get())
        .fetch_optional(&mut *tx)
        .await?;
        if active.is_none() {
            return Ok(Stale);
        }
        let charged: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM mail_send_reservations WHERE outbox_id=$1) \
             OR EXISTS(SELECT 1 FROM mail_feedback_attempts WHERE outbox_id=$1)",
        )
        .bind(claim.id)
        .fetch_one(&mut *tx)
        .await?;
        if charged {
            return Ok(Charged);
        }
        let status: Option<String> = sqlx::query_scalar(
            r#"WITH stamp AS MATERIALIZED (SELECT clock_timestamp() AS at),
            decision AS (
                SELECT m.id,s.at,s.at+($3::bigint*interval '1 second') AS next_at,
                       m.generation<$4 AND s.at+($3::bigint*interval '1 second')
                           < m.created_at+CASE WHEN m.purpose='password_reset'
                             THEN interval '10 minutes' ELSE interval '24 hours' END AS retry
                FROM mail_outbox m CROSS JOIN stamp s WHERE m.id=$1 AND m.generation=$2
                  AND m.status='leased' AND m.lease_until>s.at
            )
            UPDATE mail_outbox m SET
              status=CASE WHEN d.retry THEN 'deferred' ELSE 'failed' END,
              next_attempt_at=CASE WHEN d.retry THEN d.next_at ELSE NULL END,
              lease_until=NULL,finished_at=CASE WHEN d.retry THEN NULL ELSE d.at END
            FROM decision d WHERE m.id=d.id RETURNING m.status"#,
        )
        .bind(claim.id)
        .bind(claim.generation)
        .bind(reason.seconds())
        .bind(MAX_CLAIMS)
        .fetch_optional(&mut *tx)
        .await?;
        let decision = match status.as_deref() {
            Some("deferred") => Deferred,
            Some("failed") => Exhausted,
            None => Stale,
            _ => return Err(sqlx::Error::Protocol("invalid mail defer decision".into())),
        };
        tx.commit().await?;
        Ok(decision)
    }
}
