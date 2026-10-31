//! E-08/E-06 projection of committed social events through the shared outbox.
//! One short PostgreSQL transaction owns the event lock, projection and receipt.
use super::mail::{blocked, pair_lock, unavailable};
use crate::domain::MailError;
use sqlx::PgPool;
use std::fmt::Write;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserNotification {
    pub id: i64,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub url: String,
    pub created_at: String,
    pub is_read: bool,
}

#[derive(Clone)]
pub struct PgNotifications {
    pool: PgPool,
}

impl PgNotifications {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Consume only this feature's events. Row-level SKIP LOCKED and the
    /// source-event unique key make competing workers/restarts idempotent.
    /// No external message or notification is sent before the DB receipt.
    pub async fn deliver_pending(&self, limit: u16) -> Result<usize, MailError> {
        if !(1..=100).contains(&limit) {
            return Err(MailError::Invalid);
        }
        let mut processed = 0;
        for _ in 0..limit {
            let mut tx = self.pool.begin().await.map_err(unavailable)?;
            let event = sqlx::query_as::<_, (String, String, i64, i64, i64)>(
                "SELECT e.id::text,e.event_type,
                        (e.payload->>'actor_user_id')::bigint,
                        (e.payload->>'recipient_user_id')::bigint,
                        (e.payload->>'object_id')::bigint
                 FROM outbox_events e
                 WHERE e.delivered_at IS NULL AND e.dead_at IS NULL
                   AND e.next_attempt_at<=clock_timestamp()
                   AND e.required_consumers @> ARRAY['user_notifications']::text[]
                   AND e.event_type IN
                     ('social.mail.created','social.friendship.requested',
                      'social.friendship.accepted','social.friendship.removed')
                   AND NOT EXISTS (
                     SELECT 1 FROM event_deliveries d
                     WHERE d.event_id=e.id AND d.consumer='user_notifications')
                 ORDER BY e.created_at,e.id
                 FOR UPDATE SKIP LOCKED LIMIT 1",
            )
            .fetch_optional(&mut *tx)
            .await
            .map_err(unavailable)?;
            let Some((event_id, kind, actor, recipient, object_id)) = event else {
                tx.commit().await.map_err(unavailable)?;
                break;
            };
            // A block committed before consumption suppresses the notification,
            // never the message. Valid actor/recipient membership is authoritative.
            let user_exists: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM users WHERE id=$1) AND
                        EXISTS(SELECT 1 FROM users WHERE id=$2)",
            )
            .bind(actor)
            .bind(recipient)
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?;
            // Use the same pair lock as send/block; a committed block cannot
            // race past an already-checked notification projection.
            if user_exists && actor != recipient && actor > 0 && recipient > 0 {
                pair_lock(&mut tx, actor, recipient).await?;
            }
            let is_blocked = if user_exists && actor != recipient && actor > 0 && recipient > 0 {
                blocked(&mut tx, actor, recipient).await?
            } else {
                true
            };
            let member = if kind == "social.mail.created" {
                sqlx::query_scalar::<_, bool>(
                    "SELECT EXISTS(SELECT 1 FROM social_conversations
                     WHERE id=$1 AND $2 IN(user_low,user_high)
                       AND $3 IN(user_low,user_high) AND $2<>$3)",
                )
                .bind(object_id)
                .bind(actor)
                .bind(recipient)
                .fetch_one(&mut *tx)
                .await
                .map_err(unavailable)?
            } else {
                object_id == actor
            };
            if user_exists
                && !is_blocked
                && actor > 0
                && recipient > 0
                && actor != recipient
                && member
            {
                let name: String = sqlx::query_scalar("SELECT display_name FROM users WHERE id=$1")
                    .bind(actor)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(unavailable)?;
                let (category, title, body, url) = match kind.as_str() {
                    "social.mail.created" => (
                        "mail",
                        "Новое сообщение",
                        format!("{name} написал вам"),
                        format!("/mail/{object_id}"),
                    ),
                    "social.friendship.requested" => (
                        "friendship",
                        "Новая заявка в друзья",
                        format!("{name} хочет добавить вас в друзья."),
                        format!("/users/{}", encode_segment(&name)),
                    ),
                    "social.friendship.accepted" => (
                        "friendship",
                        "Заявка в друзья принята",
                        format!("{name} теперь у вас в друзьях."),
                        format!("/users/{}", encode_segment(&name)),
                    ),
                    "social.friendship.removed" => (
                        "friendship",
                        "Удаление из друзей",
                        format!("{name} удалил вас из друзей."),
                        format!("/users/{}", encode_segment(&name)),
                    ),
                    _ => unreachable!("event_type is closed by SQL"),
                };
                sqlx::query(
                    "INSERT INTO user_notifications
                       (recipient_user_id,actor_user_id,source_event_id,
                        kind,title,body,url,created_at)
                     SELECT $1,$2,e.id,$3,$4,$5,$6,e.created_at
                     FROM outbox_events e WHERE e.id=$7::uuid
                     ON CONFLICT(source_event_id) DO NOTHING",
                )
                .bind(recipient)
                .bind(actor)
                .bind(category)
                .bind(title)
                .bind(body)
                .bind(url)
                .bind(&event_id)
                .execute(&mut *tx)
                .await
                .map_err(unavailable)?;
            }
            sqlx::query(
                "INSERT INTO event_deliveries(consumer,event_id)
                 VALUES('user_notifications',$1::uuid) ON CONFLICT DO NOTHING",
            )
            .bind(&event_id)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
            sqlx::query(
                "UPDATE outbox_events o SET delivered_at=clock_timestamp()
                 WHERE o.id=$1::uuid AND NOT EXISTS(
                     SELECT 1 FROM unnest(o.required_consumers) AS required(consumer)
                     WHERE NOT EXISTS(
                         SELECT 1 FROM event_deliveries d
                         WHERE d.event_id=o.id AND d.consumer=required.consumer))",
            )
            .bind(&event_id)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
            tx.commit().await.map_err(unavailable)?;
            processed += 1;
        }
        Ok(processed)
    }

    /// Default inbox preserves the existing pagination and actor-only contract.
    pub async fn list(
        &self,
        actor: i64,
        before: Option<i64>,
        limit: u16,
    ) -> Result<Vec<UserNotification>, MailError> {
        self.list_section(actor, before, limit, "notifications")
            .await
    }

    /// Source notification tabs, filtered before LIMIT so later matching rows
    /// cannot be skipped by a page full of unrelated kinds.
    pub async fn list_section(
        &self,
        actor: i64,
        before: Option<i64>,
        limit: u16,
        section: &str,
    ) -> Result<Vec<UserNotification>, MailError> {
        if actor <= 0
            || before.is_some_and(|id| id <= 0)
            || !(1..=100).contains(&limit)
            || !matches!(section, "notifications" | "releases" | "system")
        {
            return Err(MailError::Invalid);
        }
        let rows = sqlx::query_as::<_, (i64, String, String, String, String, String, bool)>(
            r#"SELECT id,kind,title,body,url,
                      to_char((created_at AT TIME ZONE 'UTC')+INTERVAL '3 hours','DD.MM.YYYY HH24:MI'),
                      read_at IS NOT NULL
               FROM user_notifications
               WHERE recipient_user_id=$1 AND ($2::bigint IS NULL OR id<$2)
                 AND (
                   $4::text='notifications'
                   OR ($4='releases' AND kind IN (
                     'chapter_created','project_news','author_new_book',
                     'author_new_layer','author_new_collection'))
                   OR ($4='system' AND kind NOT IN (
                     'chapter_created','project_news','author_new_book',
                     'author_new_layer','author_new_collection',
                     'comment','comment_reply','comment_mention'))
                 )
               ORDER BY id DESC LIMIT $3"#,
        )
        .bind(actor)
        .bind(before)
        .bind(i64::from(limit))
        .bind(section)
        .fetch_all(&self.pool)
        .await
        .map_err(unavailable)?;
        Ok(rows
            .into_iter()
            .map(|r| UserNotification {
                id: r.0,
                kind: r.1,
                title: r.2,
                body: r.3,
                url: r.4,
                created_at: r.5,
                is_read: r.6,
            })
            .collect())
    }

    pub async fn mark_read(&self, actor: i64, id: i64) -> Result<bool, MailError> {
        if actor <= 0 || id <= 0 {
            return Err(MailError::Invalid);
        }
        let count = sqlx::query(
            "UPDATE user_notifications SET read_at=COALESCE(read_at,clock_timestamp())
             WHERE recipient_user_id=$1 AND id=$2",
        )
        .bind(actor)
        .bind(id)
        .execute(&self.pool)
        .await
        .map_err(unavailable)?
        .rows_affected();
        Ok(count == 1)
    }

    pub async fn mark_all_read(&self, actor: i64) -> Result<u64, MailError> {
        if actor <= 0 {
            return Err(MailError::Invalid);
        }
        sqlx::query(
            "UPDATE user_notifications SET read_at=clock_timestamp()
             WHERE recipient_user_id=$1 AND read_at IS NULL",
        )
        .bind(actor)
        .execute(&self.pool)
        .await
        .map(|result| result.rows_affected())
        .map_err(unavailable)
    }
}

/// Encode a single public profile path segment, not an untrusted redirect.
fn encode_segment(name: &str) -> String {
    let mut result = String::new();
    for &byte in name.as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            result.push(char::from(byte));
        } else {
            write!(result, "%{byte:02X}").expect("write to string");
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::encode_segment;

    #[test]
    fn profile_notification_link_is_one_encoded_path_segment() {
        assert_eq!(
            encode_segment("Алиса / Боб"),
            "%D0%90%D0%BB%D0%B8%D1%81%D0%B0%20%2F%20%D0%91%D0%BE%D0%B1"
        );
        assert_eq!(encode_segment("//evil.example"), "%2F%2Fevil.example");
    }
}
