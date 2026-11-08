//! B-owned site-moderator withdrawal of an already published cover.
use super::*;

impl PgCoverReview {
    /// Snapshot for a moderator's confirmation form. The subsequent write
    /// transaction independently rechecks role and expected revision.
    pub async fn current_active(
        &self,
        session: &[u8; 32],
        book_id: i64,
    ) -> Result<Option<i64>, CoverReviewError> {
        if book_id <= 0 {
            return Err(CoverReviewError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(|_| CoverReviewError::Unavailable)?;
        live_site_moderator(&mut tx, session).await?;
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT s.revision FROM book_cover_slots s
             JOIN books b ON b.id=s.book_id
             WHERE b.id=$1 AND NOT s.is_withdrawn",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| CoverReviewError::Unavailable)?;
        tx.commit().await.map_err(|_| CoverReviewError::Unavailable)?;
        Ok(current)
    }

    /// Withdraw a currently approved cover without hiding the Book or
    /// deleting its private immutable source. The slot revision remains
    /// monotonic, so stale workers and review forms cannot reattach it.
    pub async fn withdraw_current(
        &self,
        session: &[u8; 32],
        book_id: i64,
        expected_revision: i64,
        reason: &str,
    ) -> Result<(), CoverReviewError> {
        let reason = reason.trim();
        if book_id <= 0 || expected_revision <= 0 || reason.is_empty() || reason.len() > 2000 {
            return Err(CoverReviewError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| CoverReviewError::Unavailable)?;
        let moderator = live_site_moderator(&mut tx, session).await?;
        let book = sqlx::query("SELECT owner_user_id FROM books WHERE id=$1 FOR UPDATE")
            .bind(book_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| CoverReviewError::Unavailable)?
            .ok_or(CoverReviewError::Missing)?;
        let owner: i64 = book.try_get("owner_user_id").map_err(|_| CoverReviewError::Unavailable)?;
        if moderator == owner {
            return Err(CoverReviewError::Denied);
        }
        let current = sqlx::query(
            "SELECT job_id,revision,is_withdrawn FROM book_cover_slots
             WHERE book_id=$1 FOR UPDATE",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| CoverReviewError::Unavailable)?
        .ok_or(CoverReviewError::Missing)?;
        let revision: i64 = current.try_get("revision").map_err(|_| CoverReviewError::Unavailable)?;
        let job_id: i64 = current.try_get("job_id").map_err(|_| CoverReviewError::Unavailable)?;
        let withdrawn: bool = current.try_get("is_withdrawn").map_err(|_| CoverReviewError::Unavailable)?;
        if withdrawn || revision != expected_revision {
            return Err(CoverReviewError::Stale);
        }
        // Keep the existing job ID for audit, but revoke every B-owned public
        // projection in the same transaction. A later approval must explicitly
        // switch to another job and a higher revision.
        sqlx::query(
            "UPDATE book_cover_slots
             SET is_withdrawn=TRUE,revision=revision+1,attached_at=now()
             WHERE book_id=$1 AND revision=$2 AND NOT is_withdrawn",
        )
        .bind(book_id)
        .bind(expected_revision)
        .execute(&mut *tx)
        .await
        .map_err(|_| CoverReviewError::Unavailable)?;
        sqlx::query(
            "UPDATE book_cover_review_requests
             SET state='superseded',superseded_at=clock_timestamp()
             WHERE book_id=$1 AND state='pending'",
        )
        .bind(book_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| CoverReviewError::Unavailable)?;
        sqlx::query(
            "INSERT INTO book_cover_slot_withdrawals
             (book_id,job_id,previous_revision,new_revision,reviewer_user_id,reason)
             VALUES($1,$2,$3,$4,$5,$6)",
        )
        .bind(book_id)
        .bind(job_id)
        .bind(expected_revision)
        .bind(expected_revision + 1)
        .bind(moderator)
        .bind(reason)
        .execute(&mut *tx)
        .await
        .map_err(|_| CoverReviewError::Unavailable)?;
        tx.commit().await.map_err(|_| CoverReviewError::Unavailable)?;
        Ok(())
    }

}
