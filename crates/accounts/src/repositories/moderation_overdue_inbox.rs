//! Site-private pending overdue observations, not delivered user notifications.
//! Every request rechecks the live session/site role and the source request.
use super::{
    moderation_books::{current_moderator, ModerationBooksError},
    PgAccounts,
};

#[derive(Clone, Debug)]
pub struct OverdueInboxItem {
    pub id: i64,
    pub request_kind: String,
    pub source_id: i64,
    pub source_revision: i64,
    pub threshold_hours: i16,
    pub first_seen_utc: String,
}

impl PgAccounts {
    /// Lists only currently actionable observations, with the highest active
    /// threshold per request, before applying keyset pagination.
    /// Uses a live site moderator session; an ordinary Book reviewer has no
    /// right to this repository method.
    pub async fn moderation_overdue_inbox(
        &self,
        session: &[u8; 32],
        before: Option<i64>,
    ) -> Result<Vec<OverdueInboxItem>, ModerationBooksError> {
        if before.is_some_and(|id| id <= 0) {
            return Err(ModerationBooksError::Invalid);
        }
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        let reviewer = current_moderator(&mut tx, session).await?;
        type InboxRow = (i64, String, i64, i64, i16, String);
        let rows: Vec<InboxRow> = sqlx::query_as(
            "SELECT s.id,s.request_kind,s.source_id,s.source_revision,
                    s.threshold_hours,
                    to_char(s.first_seen_at AT TIME ZONE 'UTC','YYYY-MM-DD HH24:MI')
             FROM moderation_overdue_signals s
             WHERE s.signal_state='open'
               AND ($1::bigint IS NULL OR s.id<$1)
               AND NOT EXISTS (
                 SELECT 1 FROM moderation_overdue_signals higher
                 WHERE higher.request_kind=s.request_kind
                   AND higher.source_id=s.source_id
                   AND higher.source_revision=s.source_revision
                   AND higher.signal_state='open'
                   AND higher.threshold_hours>s.threshold_hours
               )
               AND (
                 (s.request_kind='registration' AND s.source_revision=0
                  AND EXISTS (
                    SELECT 1 FROM books b
                    WHERE b.id=s.source_id AND b.registration_requires_review
                      AND b.visibility_status='pending_moderation'
                      AND b.registration_approval_decision_id IS NULL
                      AND NOT b.is_deleted
                      AND b.created_at <= now()-make_interval(hours=>s.threshold_hours::integer)
                      AND NOT EXISTS (
                        SELECT 1 FROM book_registration_decisions d WHERE d.book_id=b.id
                      )
                  ))
                 OR
                 (s.request_kind='cover' AND s.source_revision=0
                  AND EXISTS (
                    SELECT 1 FROM book_cover_review_requests c
                    JOIN books b ON b.id=c.book_id
                    WHERE c.id=s.source_id AND c.state='pending'
                      AND NOT b.is_deleted
                      AND c.submitted_at <= now()-make_interval(hours=>s.threshold_hours::integer)
                  ))
                 OR
                 (s.request_kind='deletion'
                  AND EXISTS (
                    SELECT 1 FROM book_ordinary_deletion_requests r
                    JOIN books b ON b.id=r.book_id
                    WHERE r.book_id=s.source_id AND r.request_revision=s.source_revision
                      AND r.owner_user_id<>$2 AND NOT b.is_deleted
                      AND b.lifecycle_status='pending_deletion'
                      AND b.lifecycle_revision=r.request_revision
                      AND r.requested_at <= now()-make_interval(hours=>s.threshold_hours::integer)
                      AND NOT EXISTS (
                        SELECT 1 FROM book_ordinary_deletion_decisions d
                        WHERE d.book_id=r.book_id AND d.request_revision=r.request_revision
                      )
                  ))
               )
             ORDER BY s.id DESC LIMIT 40",
        )
        .bind(before)
        .bind(reviewer)
        .fetch_all(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        Ok(rows
            .into_iter()
            .map(|r| OverdueInboxItem {
                id: r.0,
                request_kind: r.1,
                source_id: r.2,
                source_revision: r.3,
                threshold_hours: r.4,
                first_seen_utc: r.5,
            })
            .collect())
    }
}
