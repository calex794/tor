//! Read-only, bounded inventory of historical notification snapshot markers.
//! No raw text, actor identifiers, addresses, or URLs leave the database.
use super::{unavailable, PgNotifications};
use crate::domain::MailError;

/// Aggregate counts over one bounded primary-key interval. Markers are
/// indicators, not proof that a notification contains a private mailbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacyNotificationAuditBatch {
    pub last_scanned_id: Option<i64>,
    pub scanned: i64,
    pub marked: i64,
    pub actor_event_matched: i64,
}

impl PgNotifications {
    /// Capture an upper bound once for an operator's finite, read-only scan.
    /// This is not a transaction snapshot: a later pass may find concurrent
    /// writes in the scanned interval. No HTTP or periodic caller is enabled.
    pub async fn legacy_notification_audit_high_watermark(&self) -> Result<i64, MailError> {
        sqlx::query_scalar("SELECT COALESCE(max(id),0) FROM user_notifications")
            .fetch_one(&self.pool)
            .await
            .map_err(unavailable)
    }

    /// Scan at most `limit` existing rows in (after_id, through_id].
    /// Advance to last_scanned_id, or stop when None. The primary-key range
    /// avoids repeating a full-table scan for sparse marker candidates.
    /// Match only the eight known actor-event types and exact stored actor
    /// and recipient bindings. Matched is NOT an authorization to rewrite.
    pub async fn audit_legacy_notification_batch(
        &self,
        after_id: i64,
        through_id: i64,
        limit: u16,
    ) -> Result<LegacyNotificationAuditBatch, MailError> {
        if after_id < 0 || through_id < after_id || !(1..=1000).contains(&limit) {
            return Err(MailError::Invalid);
        }
        let (last_scanned_id, scanned, marked, actor_event_matched) =
            sqlx::query_as::<_, (Option<i64>, i64, i64, i64)>(
                "WITH batch AS MATERIALIZED (
                    SELECT id,kind,recipient_user_id,actor_user_id,source_event_id,
                           (strpos(title,'@')>0 OR strpos(body,'@')>0
                            OR strpos(url,'@')>0 OR strpos(lower(url),'%40')>0) AS marked
                    FROM user_notifications
                    WHERE id>$1 AND id<=$2
                    ORDER BY id LIMIT $3
                 )
                 SELECT max(n.id), count(*),
                        count(*) FILTER (WHERE n.marked),
                        count(*) FILTER (
                          WHERE n.marked AND e.id IS NOT NULL
                            AND e.payload->>'actor_user_id'=n.actor_user_id::text
                            AND e.payload->>'recipient_user_id'=n.recipient_user_id::text
                            AND n.kind=CASE e.event_type
                              WHEN 'social.mail.created' THEN 'mail'
                              WHEN 'social.group_mail.created' THEN 'mail'
                              WHEN 'social.group_creator_transfer.proposed' THEN 'mail'
                              WHEN 'social.friendship.requested' THEN 'friendship'
                              WHEN 'social.friendship.accepted' THEN 'friendship'
                              WHEN 'social.friendship.removed' THEN 'friendship'
                              WHEN 'reader.chapter.released' THEN 'chapter_created'
                              WHEN 'reader.chapter.updated' THEN 'chapter_updated'
                            END
                        )
                 FROM batch n
                 LEFT JOIN outbox_events e ON n.marked AND e.id=n.source_event_id",
            )
            .bind(after_id)
            .bind(through_id)
            .bind(i64::from(limit))
            .fetch_one(&self.pool)
            .await
            .map_err(unavailable)?;
        Ok(LegacyNotificationAuditBatch {
            last_scanned_id,
            scanned,
            marked,
            actor_event_matched,
        })
    }
}
