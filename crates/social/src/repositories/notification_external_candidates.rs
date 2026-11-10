//! S013/S018: read-only preflight for future content-notification transport.
//! A stored delivery mode is NEVER channel consent or a sending capability.
//! Do not use this result as a long-lived grant: recheck live policy at send time.
#![forbid(unsafe_code)]
use super::{read_policy, unavailable, PgNotifications};
use crate::domain::MailError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalReleasePreview {
    pub notification_id: i64,
    pub message: &'static str,
    pub url: &'static str,
}

impl PgNotifications {
    /// Return reference-only candidate IDs for the actor's unread, still-
    /// accessible release notices. No text, title, author name, Book ID,
    /// private path or stored notification payload leaves the database.
    ///
    /// The preferences row is a coarse eligibility filter, NOT permission to
    /// send mail/Telegram/push; the external transport must independently
    /// verify channel consent and recheck access just before transmission.
    pub async fn external_release_previews(
        &self,
        actor: i64,
        limit: u16,
    ) -> Result<Vec<ExternalReleasePreview>, MailError> {
        if actor <= 0 || !(1..=50).contains(&limit) {
            return Err(MailError::Invalid);
        }
        let sql = format!(
            r#"SELECT n.id FROM user_notifications n
               JOIN users recipient ON recipient.id=n.recipient_user_id
                 AND recipient.account_state='active'
               JOIN reader_notification_delivery_preferences p
                 ON p.user_id=n.recipient_user_id
               WHERE n.recipient_user_id=$1 AND n.kind='chapter_created'
                 AND n.read_at IS NULL
                 AND p.delivery_mode IN ('immediate','smart_time','daily_digest')
                 AND {}
               ORDER BY n.id DESC LIMIT $2"#,
            read_policy::VISIBLE
        );
        let ids: Vec<i64> = sqlx::query_scalar(&sql)
            .bind(actor)
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await
            .map_err(unavailable)?;
        Ok(ids
            .into_iter()
            .map(|notification_id| ExternalReleasePreview {
                notification_id,
                // Reference-only by construction, even if stored n.body is
                // spoiled, malicious or from a paid/unavailable chapter.
                message: "В вашей библиотеке появились обновления.",
                url: "/notifications?tab=releases",
            })
            .collect())
    }
}
