//! E-03: recipient verification against visible users and current discussion.
use crate::domain::mentions::MentionKey;
use sqlx::PgPool;

#[derive(Clone)]
pub struct PgMentions {
    pool: PgPool,
}

impl PgMentions {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Only people who participated in this visible book discussion may be
    /// resolved. Friend discovery needs the later A-line relationship contract.
    pub async fn verified_candidates(
        &self,
        book_id: i64,
        keys: &[MentionKey],
    ) -> Result<Vec<(MentionKey, i64)>, sqlx::Error> {
        let mut out = Vec::new();
        for key in keys.iter().take(100) {
            let target = match key {
                MentionKey::User(user_id) if *user_id > 0 => {
                    sqlx::query_scalar::<_, i64>(
                        "SELECT u.id FROM users u
                         WHERE u.id=$1 AND EXISTS (
                           SELECT 1 FROM discussion_comments c
                           JOIN books b ON b.id=c.book_id
                           WHERE c.book_id=$2 AND c.author_user_id=u.id
                             AND c.visibility='visible' AND b.visibility='public'
                             AND NOT b.admin_hold AND NOT b.is_deleted
                         )",
                    )
                    .bind(user_id)
                    .bind(book_id)
                    .fetch_optional(&self.pool)
                    .await?
                }
                MentionKey::Handle(handle) => {
                    sqlx::query_scalar::<_, i64>(
                        "SELECT u.id FROM discussion_mention_handles h
                         JOIN users u ON u.id=h.user_id
                         WHERE h.handle=$1 AND EXISTS (
                           SELECT 1 FROM discussion_comments c
                           JOIN books b ON b.id=c.book_id
                           WHERE c.book_id=$2 AND c.author_user_id=u.id
                             AND c.visibility='visible' AND b.visibility='public'
                             AND NOT b.admin_hold AND NOT b.is_deleted
                         )",
                    )
                    .bind(handle)
                    .bind(book_id)
                    .fetch_optional(&self.pool)
                    .await?
                }
                _ => None,
            };
            if let Some(id) = target {
                out.push((key.clone(), id));
            }
        }
        Ok(out)
    }

    /// A separate, explicitly private discovery query. Never list arbitrary users.
    pub async fn discover_participants(
        &self,
        actor_id: i64,
        book_id: i64,
        query: &str,
    ) -> Result<Vec<(i64, String)>, sqlx::Error> {
        if actor_id <= 0 || book_id <= 0 || query.len() < 2 || query.len() > 32 {
            return Ok(Vec::new());
        }
        sqlx::query_as::<_, (i64, String)>(
            "SELECT DISTINCT h.user_id,h.handle
             FROM discussion_mention_handles h
             JOIN discussion_comments c ON c.author_user_id=h.user_id
             JOIN books b ON b.id=c.book_id
             WHERE c.book_id=$1 AND c.visibility='visible'
               AND b.visibility='public' AND NOT b.admin_hold AND NOT b.is_deleted
               AND h.user_id<>$2 AND h.handle LIKE $3
             ORDER BY h.handle LIMIT 20",
        )
        .bind(book_id)
        .bind(actor_id)
        .bind(format!("{}%", query.to_ascii_lowercase()))
        .fetch_all(&self.pool)
        .await
    }
}
