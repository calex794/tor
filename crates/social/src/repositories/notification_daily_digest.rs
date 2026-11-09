//! S018: private, bounded, live-authorized chapter digest for the preceding UTC day.
//! This is an on-site read model, not a scheduled email or an outbox publisher.
#![forbid(unsafe_code)]
use super::{read_policy, unavailable, PgNotifications};
use crate::domain::MailError;

/// Read-only previous completed UTC day's chapter-release digest.
/// Never includes chapter bodies, message snippets, mail, or a new delivery grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyChapterDigestEntry {
    pub notification_id: i64,
    pub chapter_id: i64,
    pub book_title: String,
    pub released_utc: String,
}

impl PgNotifications {
    /// Only a completed UTC calendar day; current-day alerts must not be
    /// represented as an already-produced digest. The existing durable bell
    /// is the source of truth and remains readable regardless of future
    /// external delivery preference. No new messages are dispatched.
    ///
    /// The S015 live entitlement guard is reused INSIDE SQL before LIMIT;
    /// this cannot revive hidden/age-revoked releases. No n.body or chapter
    /// text is selected, and destinations are built from safe numeric IDs.
    pub async fn previous_day_chapter_digest(
        &self,
        actor: i64,
        limit: u16,
    ) -> Result<Vec<DailyChapterDigestEntry>, MailError> {
        if actor <= 0 || !(1..=51).contains(&limit) {
            return Err(MailError::Invalid);
        }
        let sql = format!(
            r#"SELECT n.id,c.id,b.title,
                      to_char(n.created_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI')
               FROM user_notifications n
               JOIN outbox_events e ON e.id=n.source_event_id
                 AND e.event_type='reader.chapter.released'
               JOIN chapters c ON c.id=e.aggregate_id
               JOIN books b ON b.id=c.book_id
               WHERE n.recipient_user_id=$1 AND n.kind='chapter_created'
                 AND n.created_at >=
                   (((clock_timestamp() AT TIME ZONE 'UTC')::date - 1)::timestamp
                    AT TIME ZONE 'UTC')
                 AND n.created_at <
                   ((clock_timestamp() AT TIME ZONE 'UTC')::date::timestamp
                    AT TIME ZONE 'UTC')
                 AND {}
               ORDER BY n.id DESC LIMIT $2"#,
            read_policy::VISIBLE
        );
        let rows: Vec<(i64, i64, String, String)> = sqlx::query_as(&sql)
            .bind(actor)
            .bind(i64::from(limit))
            .fetch_all(&self.pool)
            .await
            .map_err(unavailable)?;
        Ok(rows
            .into_iter()
            .map(|(notification_id, chapter_id, book_title, released_utc)| {
                DailyChapterDigestEntry {
                    notification_id,
                    chapter_id,
                    book_title,
                    released_utc,
                }
            })
            .collect())
    }

}
