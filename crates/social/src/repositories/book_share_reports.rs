//! S049: administrator-only review of existing S017 book-share complaints.
//! A complaint about a recommendation never automatically holds a Book,
//! suspends an account, or grants access to private/paid content.
#![forbid(unsafe_code)]
use super::book_shares::BookShareError;
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Clone)]
pub struct PgBookShareReports {
    pool: PgPool,
}

#[derive(Debug)]
pub struct PendingBookShareReport {
    pub share_id: i64,
    pub book_id: i64,
    pub sender_id: i64,
    pub recipient_id: i64,
    pub reason: String,
    pub details: String,
    pub message: String,
}

/// Check the live administrator role under the same transaction as the
/// read or decision. A revoked session or role cannot use a stale HTTP page.
async fn administrator(
    tx: &mut Transaction<'_, Postgres>,
    session_digest: &[u8; 32],
) -> Result<i64, BookShareError> {
    let actor: Option<i64> = sqlx::query_scalar(
        "SELECT u.id FROM sessions s
         JOIN users u ON u.id=s.user_id
         JOIN account_roles r ON r.user_id=u.id
         WHERE s.token_hash=$1 AND s.revoked_at IS NULL
           AND s.expires_at>now() AND u.account_state='active'
           AND r.role='administrator' AND r.is_active
         FOR SHARE OF s,u,r",
    )
    .bind(session_digest.as_slice())
    .fetch_optional(&mut **tx)
    .await?;
    actor.ok_or(BookShareError::Forbidden)
}

impl PgBookShareReports {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Return a bounded queue without Book titles or paid chapter text.
    /// The original share message is escaped by the private HTTP renderer.
    pub async fn pending(
        &self,
        session_digest: &[u8; 32],
        limit: u16,
    ) -> Result<Vec<PendingBookShareReport>, BookShareError> {
        if !(1..=50).contains(&limit) {
            return Err(BookShareError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        administrator(&mut tx, session_digest).await?;
        let rows: Vec<(i64, i64, i64, i64, String, String, String)> = sqlx::query_as(
            "SELECT r.share_id,s.book_id,s.sender_user_id,s.recipient_user_id,
                    r.reason,r.details,s.message
             FROM social_book_share_reports r
             JOIN social_book_shares s ON s.id=r.share_id
             WHERE r.state='pending'
             ORDER BY r.reported_at,r.share_id LIMIT $1",
        )
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .map(
                |(share_id, book_id, sender_id, recipient_id, reason, details, message)| {
                    PendingBookShareReport {
                        share_id,
                        book_id,
                        sender_id,
                        recipient_id,
                        reason,
                        details,
                        message,
                    }
                },
            )
            .collect())
    }

    /// Exactly one decision per complaint. A duplicate or racing submission
    /// cannot rewrite history; both the decision and state update are atomic.
    pub async fn decide(
        &self,
        session_digest: &[u8; 32],
        share_id: i64,
        decision: &str,
        note: &str,
    ) -> Result<(), BookShareError> {
        let note = note.trim();
        if share_id <= 0
            || !matches!(decision, "reviewed" | "dismissed")
            || note.is_empty()
            || note.chars().count() > 500
            || note.contains('\0')
        {
            return Err(BookShareError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let reviewer = administrator(&mut tx, session_digest).await?;
        let state: Option<String> = sqlx::query_scalar(
            "SELECT state FROM social_book_share_reports
             WHERE share_id=$1 FOR UPDATE",
        )
        .bind(share_id)
        .fetch_optional(&mut *tx)
        .await?;
        match state.as_deref() {
            None => return Err(BookShareError::NotFound),
            Some("pending") => {}
            _ => return Err(BookShareError::Conflict),
        }
        sqlx::query(
            "INSERT INTO social_book_share_report_reviews
             (share_id,reviewer_user_id,decision,note)
             VALUES($1,$2,$3,$4)",
        )
        .bind(share_id)
        .bind(reviewer)
        .bind(decision)
        .bind(note)
        .execute(&mut *tx)
        .await?;
        let updated = sqlx::query(
            "UPDATE social_book_share_reports SET state=$2
             WHERE share_id=$1 AND state='pending'",
        )
        .bind(share_id)
        .bind(decision)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(BookShareError::Conflict);
        }
        tx.commit().await?;
        Ok(())
    }
}
