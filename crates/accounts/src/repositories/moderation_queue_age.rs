//! Site-private, read-only operational age counters for the three real queues.
//! These are on-screen triage signals, not notifications or an accepted SLA.
use super::{
    moderation_books::{current_moderator, ModerationBooksError},
    PgAccounts,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WaitingCounts {
    pub at_least_24h: i64,
    pub at_least_72h: i64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModerationQueueAgeSummary {
    pub registrations: WaitingCounts,
    pub covers: WaitingCounts,
    pub deletions: WaitingCounts,
}

impl PgAccounts {
    /// The reviewer must hold a live site role for the entire read transaction.
    /// Registration uses initial Book creation only; it cannot represent the
    /// submission timestamp of a future separate re-registration request.
    /// Deletion omits requests owned by the reviewer, just like their queue.
    pub async fn moderation_queue_age_summary(
        &self,
        session: &[u8; 32],
    ) -> Result<ModerationQueueAgeSummary, ModerationBooksError> {
        let mut tx = self
            .pool()
            .begin()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        let reviewer = current_moderator(&mut tx, session).await?;
        let counts: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
            "WITH pending(kind, submitted_at) AS (
                SELECT 'registration'::text, b.created_at
                FROM books b
                WHERE b.registration_requires_review
                  AND b.visibility_status='pending_moderation'
                  AND b.registration_approval_decision_id IS NULL
                  AND NOT b.is_deleted
                UNION ALL
                SELECT 'cover'::text, c.submitted_at
                FROM book_cover_review_requests c
                WHERE c.state='pending'
                UNION ALL
                SELECT 'deletion'::text, r.requested_at
                FROM book_ordinary_deletion_requests r
                JOIN books b ON b.id=r.book_id
                WHERE b.lifecycle_status='pending_deletion'
                  AND b.lifecycle_revision=r.request_revision
                  AND NOT b.is_deleted
                  AND r.owner_user_id<>$1
                  AND NOT EXISTS (
                      SELECT 1 FROM book_ordinary_deletion_decisions d
                      WHERE d.book_id=r.book_id AND d.request_revision=r.request_revision
                  )
            )
            SELECT
              count(*) FILTER (
                WHERE kind='registration' AND submitted_at<=now()-interval '24 hours'),
              count(*) FILTER (
                WHERE kind='registration' AND submitted_at<=now()-interval '72 hours'),
              count(*) FILTER (
                WHERE kind='cover' AND submitted_at<=now()-interval '24 hours'),
              count(*) FILTER (
                WHERE kind='cover' AND submitted_at<=now()-interval '72 hours'),
              count(*) FILTER (
                WHERE kind='deletion' AND submitted_at<=now()-interval '24 hours'),
              count(*) FILTER (
                WHERE kind='deletion' AND submitted_at<=now()-interval '72 hours')
            FROM pending",
        )
        .bind(reviewer)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| ModerationBooksError::Unavailable)?;
        tx.commit()
            .await
            .map_err(|_| ModerationBooksError::Unavailable)?;
        Ok(ModerationQueueAgeSummary {
            registrations: WaitingCounts {
                at_least_24h: counts.0,
                at_least_72h: counts.1,
            },
            covers: WaitingCounts {
                at_least_24h: counts.2,
                at_least_72h: counts.3,
            },
            deletions: WaitingCounts {
                at_least_24h: counts.4,
                at_least_72h: counts.5,
            },
        })
    }
}
