//! Explicit, bounded repair of known system-generated notification snapshots.
//! No public endpoint, automatic scheduling, or production invocation.
use super::{
    actor_text,
    privacy_audit::{CANONICAL_EVENT_SQL, MARKER_SQL},
    unavailable, PgNotifications,
};
use crate::domain::MailError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacyNotificationRepairBatch {
    pub last_scanned_id: Option<i64>,
    pub scanned: usize,
    pub eligible: usize,
    pub repaired: u64,
}

#[derive(sqlx::FromRow)]
struct Candidate {
    id: i64,
    kind: String,
    event_type: String,
    actor_user_id: i64,
    aggregate_id: i64,
    object_id: Option<String>,
    public_name: Option<String>,
    handle: Option<String>,
}

impl Candidate {
    fn replacement(&self) -> Option<actor_text::ActorText> {
        let object_id = self.object_id.as_deref()?.parse::<i64>().ok()?;
        if object_id <= 0
            || (self.kind == "friendship" && object_id != self.actor_user_id)
            || (matches!(self.kind.as_str(), "chapter_created" | "chapter_updated")
                && object_id != self.aggregate_id)
        {
            return None;
        }
        actor_text::project(
            &self.event_type,
            self.public_name.as_deref(),
            self.handle.as_deref(),
            Some(object_id),
        )
    }
}

impl PgNotifications {
    /// Repair at most 100 existing rows in one explicit transaction. Do not
    /// skip locked candidates and advance the cursor: a timeout rolls back
    /// the whole batch and the caller must retry the same interval.
    /// The caller must audit again after a complete pass; concurrent inserts
    /// with earlier allocated IDs can appear behind the returned cursor.
    pub async fn repair_legacy_notification_batch(
        &self,
        after_id: i64,
        through_id: i64,
        limit: u16,
    ) -> Result<LegacyNotificationRepairBatch, MailError> {
        if after_id < 0 || through_id < after_id || !(1..=100).contains(&limit) {
            return Err(MailError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("SET LOCAL lock_timeout='500ms'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("SET LOCAL statement_timeout='2s'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        // Capture exact IDs, not just a range: new rows committed between
        // statements must not expand the size of the locked/write batch.
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM user_notifications
             WHERE id>$1 AND id<=$2 ORDER BY id LIMIT $3",
        )
        .bind(after_id)
        .bind(through_id)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await
        .map_err(unavailable)?;
        let mut result = LegacyNotificationRepairBatch {
            last_scanned_id: ids.last().copied(),
            scanned: ids.len(),
            eligible: 0,
            repaired: 0,
        };
        if !ids.is_empty() {
            let sql = format!(
                "SELECT n.id,n.kind,e.event_type,n.actor_user_id,e.aggregate_id,
                        e.payload->>'object_id' AS object_id,
                        CASE WHEN u.account_state='active'
                               AND strpos(lower(u.display_name),u.email_normalized)=0
                             THEN u.display_name END AS public_name,
                        CASE WHEN u.account_state='active' THEN s.handle END AS handle
                 FROM user_notifications n
                 JOIN outbox_events e ON e.id=n.source_event_id
                 LEFT JOIN users u ON u.id=n.actor_user_id
                 LEFT JOIN account_settings s ON s.user_id=u.id
                 WHERE n.id=ANY($1) AND {MARKER_SQL} AND {CANONICAL_EVENT_SQL}
                   AND e.schema_version=1
                   AND EXISTS (SELECT 1 FROM event_deliveries d
                               WHERE d.event_id=e.id AND d.consumer='user_notifications')
                 ORDER BY n.id FOR UPDATE OF n",
            );
            let rows = sqlx::query_as::<_, Candidate>(sqlx::AssertSqlSafe(sql))
                .bind(&ids)
                .fetch_all(&mut *tx)
                .await
                .map_err(unavailable)?;
            result.eligible = rows.len();
            let mut update_ids = Vec::new();
            let mut titles = Vec::new();
            let mut bodies = Vec::new();
            let mut urls = Vec::new();
            for row in rows {
                if let Some((kind, title, body, url)) = row.replacement() {
                    if kind != row.kind {
                        continue;
                    }
                    update_ids.push(row.id);
                    titles.push(title.to_owned());
                    bodies.push(body);
                    urls.push(url);
                }
            }
            if !update_ids.is_empty() {
                result.repaired = sqlx::query(
                    "UPDATE user_notifications n
                     SET title=r.title,body=r.body,url=r.url
                     FROM unnest($1::bigint[],$2::text[],$3::text[],$4::text[])
                        AS r(id,title,body,url)
                     WHERE n.id=r.id",
                )
                .bind(&update_ids)
                .bind(&titles)
                .bind(&bodies)
                .bind(&urls)
                .execute(&mut *tx)
                .await
                .map_err(unavailable)?
                .rows_affected();
                if result.repaired != update_ids.len() as u64 {
                    return Err(MailError::Unavailable);
                }
            }
        }
        tx.commit().await.map_err(unavailable)?;
        Ok(result)
    }
}
