//! Resolve only committed administrator replies owned by the intended account.
use super::postgres::{unavailable, PgSupport};
use crate::domain::SupportError;
use tor_contracts::mail::{MailIntent, MailPurpose};

impl PgSupport {
    /// No conversation body or mailbox crosses this boundary. A guest,
    /// another account, a user message or an unaudited reply cannot qualify.
    pub async fn owns_mail_reply(&self, intent: MailIntent) -> Result<bool, SupportError> {
        if intent.purpose != MailPurpose::SupportReply {
            return Ok(false);
        }
        sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM support_messages m
                JOIN support_conversations c ON c.id=m.conversation_id
                JOIN admin_action_audit a ON a.resource_id=m.id
                 AND a.action='support.reply' AND a.outcome='success'
                 AND a.resource_type='support_message'
                WHERE m.id=$1 AND c.requester_user_id=$2
                  AND c.guest_identity_hash IS NULL AND c.expires_at IS NULL
                  AND m.sender_kind='admin' AND m.sender_user_id=a.actor_id
            )",
        )
        .bind(intent.event_id.get())
        .bind(intent.recipient.get())
        .fetch_one(&self.pool)
        .await
        .map_err(unavailable)
    }
}
