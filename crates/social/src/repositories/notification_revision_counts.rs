//! Actor-scoped V2 unread summary. V1 still belongs to C-16 first releases.
use super::{read_policy, unavailable, PgNotifications};
use crate::domain::MailError;

impl PgNotifications {
    /// Current, actor-visible unacknowledged V2 notices. LIMIT is applied
    /// after the live moderation/age/follow/version guard.
    pub async fn unread_chapter_revisions_capped(&self, actor: i64) -> Result<u16, MailError> {
        if actor <= 0 {
            return Err(MailError::Invalid);
        }
        let sql = format!(
            "SELECT count(*)::bigint FROM (
                SELECT 1 FROM user_notifications n
                WHERE n.recipient_user_id=$1 AND n.kind='chapter_updated'
                  AND n.read_at IS NULL AND {}
                LIMIT 100
             ) visible",
            read_policy::VISIBLE
        );
        // Only the static read_policy::VISIBLE predicate is interpolated; values are bound.
        let count: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(actor)
            .fetch_one(&self.pool)
            .await
            .map_err(unavailable)?;
        u16::try_from(count).map_err(|_| MailError::Unavailable)
    }
}
