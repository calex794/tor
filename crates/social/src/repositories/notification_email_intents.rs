//! S018: stage bounded, reference-only *intentions* from accepted E-08 bells.
//! This is NOT a sender, not a second event bus and not a transport grant.
//! A future provider must atomically claim, revalidate ACL/consent and use
//! a stable idempotency key before external handoff. No worker calls this yet.
#![forbid(unsafe_code)]
use super::{read_policy, unavailable, PgNotifications};
use crate::domain::MailError;

impl PgNotifications {
    /// Actor-scoped, idempotent staging for the immediate email mode ONLY.
    /// No delivery, claims, retries, email addresses or plaintext is exposed.
    /// Locked-out content and lost age/access rights cannot enter the queue.
    pub async fn stage_immediate_email_intents(
        &self,
        actor: i64,
        limit: u16,
    ) -> Result<usize, MailError> {
        if actor <= 0 || !(1..=50).contains(&limit) {
            return Err(MailError::Invalid);
        }
        let sql = format!(
            r#"WITH allowed AS (
                 SELECT n.id AS notification_id, n.recipient_user_id
                 FROM user_notifications n
                 JOIN users recipient ON recipient.id=n.recipient_user_id
                   AND recipient.account_state='active'
                 JOIN reader_notification_delivery_preferences p
                   ON p.user_id=n.recipient_user_id
                   AND p.delivery_mode='immediate'
                 JOIN reader_notification_email_proofs consent
                   ON consent.user_id=n.recipient_user_id
                   AND consent.verified_at IS NOT NULL
                   AND consent.opted_in_at IS NOT NULL
                   AND consent.revoked_at IS NULL
                   AND consent.email_sha256 =
                       sha256(convert_to(recipient.email_normalized,'UTF8'))
                 WHERE n.recipient_user_id=$1
                   AND n.kind='chapter_created'
                   AND n.read_at IS NULL
                   AND NOT EXISTS (
                       SELECT 1 FROM reader_email_notification_intents staged
                       WHERE staged.notification_id=n.id
                   )
                   AND {}
                 ORDER BY n.id DESC LIMIT $2
               )
               INSERT INTO reader_email_notification_intents
                   (notification_id,recipient_user_id,delivery_mode)
               SELECT notification_id,recipient_user_id,'immediate' FROM allowed
               ON CONFLICT(notification_id) DO NOTHING
               RETURNING id"#,
            read_policy::VISIBLE,
        );
        let created: Vec<i64> = sqlx::query_scalar(&sql)
            .bind(actor)
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await
            .map_err(unavailable)?;
        Ok(created.len())
    }
}
