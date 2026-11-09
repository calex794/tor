//! S049: bounded, role-scoped history of immutable Book complaint reviews.
//! No private Book/comment body, payment facts or chapter metadata are returned.
use super::{site_reviewer, BookShareError, PgBookContentReports};
use sqlx::Row;

#[derive(Debug)]
pub struct ReviewedBookContentReport {
    pub event_id: i64,
    pub report_id: i64,
    pub book_id: i64,
    pub comment_id: Option<i64>,
    pub reviewer_id: i64,
    pub decision: String,
    pub note: String,
    pub moderation_action: String,
    pub decided_utc: String,
}

#[derive(Debug)]
pub struct ReviewedBookContentReportPage {
    pub items: Vec<ReviewedBookContentReport>,
    pub next_cursor: Option<i64>,
}

impl PgBookContentReports {
    /// An active administrator sees all review history. A site moderator may
    /// inspect only comment reports, matching the live pending queue's scope.
    pub async fn review_history(
        &self,
        session_digest: &[u8; 32],
        before_id: Option<i64>,
        limit: u16,
    ) -> Result<ReviewedBookContentReportPage, BookShareError> {
        if !(1..=50).contains(&limit) || before_id.is_some_and(|id| id <= 0) {
            return Err(BookShareError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let (_, is_admin) = site_reviewer(&mut tx, session_digest).await?;
        let rows = sqlx::query(
            "SELECT e.id AS event_id, e.report_id, r.book_id, r.comment_id,
                    e.reviewer_user_id, e.decision, e.note, e.moderation_action,
                    to_char(e.decided_at AT TIME ZONE 'UTC',
                            'YYYY-MM-DD HH24:MI:SS') AS decided_utc
             FROM social_book_content_review_events e
             JOIN social_book_content_reports r ON r.id=e.report_id
             WHERE ($1::BIGINT IS NULL OR e.id < $1)
               AND ($3::boolean OR r.comment_id IS NOT NULL)
             ORDER BY e.id DESC LIMIT $2",
        )
        .bind(before_id)
        .bind(i64::from(limit) + 1)
        .bind(is_admin)
        .fetch_all(&mut *tx)
        .await?;
        let has_more = rows.len() > usize::from(limit);
        let items = rows
            .into_iter()
            .take(usize::from(limit))
            .map(|row| {
                Ok(ReviewedBookContentReport {
                    event_id: row.try_get("event_id")?,
                    report_id: row.try_get("report_id")?,
                    book_id: row.try_get("book_id")?,
                    comment_id: row.try_get("comment_id")?,
                    reviewer_id: row.try_get("reviewer_user_id")?,
                    decision: row.try_get("decision")?,
                    note: row.try_get("note")?,
                    moderation_action: row.try_get("moderation_action")?,
                    decided_utc: row.try_get("decided_utc")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        let next_cursor = if has_more {
            items.last().map(|item| item.event_id)
        } else {
            None
        };
        tx.commit().await?;
        Ok(ReviewedBookContentReportPage { items, next_cursor })
    }
}
