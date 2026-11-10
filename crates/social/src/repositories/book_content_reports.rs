//! S049: authenticated Book complaints, administrator-only review.
//! Reports are moderation evidence only, not automatic holds or read grants.
#![forbid(unsafe_code)]
use super::book_shares::BookShareError;

mod history;
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Clone)]
pub struct PgBookContentReports {
    pool: PgPool,
}

#[derive(Debug)]
pub struct PendingBookContentReport {
    pub id: i64,
    pub book_id: i64,
    pub comment_id: Option<i64>,
    pub reporter_id: i64,
    pub reason: String,
    pub details: String,
}

fn allowed(book_level: &str, adult: bool, pro_adult: bool) -> bool {
    match book_level {
        "basic" => true,
        "adult" => adult,
        "pro_adult" => pro_adult,
        _ => false,
    }
}

fn valid_command_key(command: &str) -> bool {
    command.len() == 43
        && command
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Only the global administrator reviews content reports; moderators handle
/// Book registration rather than content/comments. Lock live role until commit.
async fn administrator(
    tx: &mut Transaction<'_, Postgres>,
    digest: &[u8; 32],
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
    .bind(digest.as_slice())
    .fetch_optional(&mut **tx)
    .await?;
    actor.ok_or(BookShareError::Forbidden)
}

impl PgBookContentReports {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// A Book comment is reportable only if it is visible in this public
    /// discussion. No chapter-edition or paid content is exposed by ID.
    pub async fn reportable_comment(
        &self,
        book_id: i64,
        comment_id: i64,
    ) -> Result<bool, BookShareError> {
        if book_id <= 0 || comment_id <= 0 {
            return Err(BookShareError::NotFound);
        }
        sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM discussion_comments
             WHERE id=$1 AND book_id=$2 AND chapter_id IS NULL
               AND version_id IS NULL AND visibility='visible')",
        )
        .bind(comment_id)
        .bind(book_id)
        .fetch_one(&self.pool)
        .await
        .map_err(BookShareError::from)
    }

    /// Show only Books that the *live session* can already see on the public
    /// reader. No private, hidden, suspended or age-ineligible Book title leaks.
    pub async fn reportable_title(
        &self,
        session_digest: &[u8; 32],
        book_id: i64,
    ) -> Result<String, BookShareError> {
        if book_id <= 0 {
            return Err(BookShareError::NotFound);
        }
        let title: Option<String> = sqlx::query_scalar(
            "SELECT b.title FROM books b
             JOIN sessions s ON s.token_hash=$2
               AND s.revoked_at IS NULL AND s.expires_at>now()
             JOIN users u ON u.id=s.user_id AND u.account_state='active'
             WHERE b.id=$1 AND b.visibility='public'
               AND b.visibility_status='active'
               AND b.lifecycle_status IN ('in_progress','completed','abandoned','on_hiatus')
               AND NOT b.is_deleted AND NOT b.admin_hold
               AND (b.access_level='basic'
                 OR (b.access_level='adult' AND u.adult_enabled)
                 OR (b.access_level='pro_adult' AND EXISTS(
                     SELECT 1 FROM user_level_grants g
                     WHERE g.user_id=u.id AND g.level='pro_adult' AND g.is_active)))",
        )
        .bind(book_id)
        .bind(session_digest.as_slice())
        .fetch_optional(&self.pool)
        .await?;
        title.ok_or(BookShareError::NotFound)
    }

    /// Serializing by reporter-user row enforces a global submission quota even
    /// for concurrent Book reports. Book and age facts are locked before writes.
    pub async fn submit(
        &self,
        session_digest: &[u8; 32],
        book_id: i64,
        reason: &str,
        details: &str,
        command_key: &str,
    ) -> Result<i64, BookShareError> {
        self.submit_target(session_digest, book_id, None, reason, details, command_key)
            .await
    }

    pub async fn submit_comment(
        &self,
        session_digest: &[u8; 32],
        book_id: i64,
        comment_id: i64,
        reason: &str,
        details: &str,
        command_key: &str,
    ) -> Result<i64, BookShareError> {
        self.submit_target(
            session_digest,
            book_id,
            Some(comment_id),
            reason,
            details,
            command_key,
        )
        .await
    }

    async fn submit_target(
        &self,
        session_digest: &[u8; 32],
        book_id: i64,
        comment_id: Option<i64>,
        reason: &str,
        details: &str,
        command_key: &str,
    ) -> Result<i64, BookShareError> {
        let details = details.trim();
        if book_id <= 0
            || comment_id.is_some_and(|id| id <= 0)
            || !matches!(reason, "spam" | "abuse" | "misleading" | "other")
            || details.chars().count() > 500
            || details.contains('\0')
            || !valid_command_key(command_key)
        {
            return Err(BookShareError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let book_level: Option<String> = sqlx::query_scalar(
            "SELECT access_level FROM books
             WHERE id=$1 AND visibility='public'
               AND visibility_status='active'
               AND lifecycle_status IN ('in_progress','completed','abandoned','on_hiatus')
               AND NOT is_deleted AND NOT admin_hold FOR SHARE",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await?;
        let book_level = book_level.ok_or(BookShareError::NotFound)?;
        let viewer: Option<(i64, bool)> = sqlx::query_as(
            "SELECT u.id,u.adult_enabled FROM users u
             JOIN sessions s ON s.user_id=u.id
             WHERE s.token_hash=$1 AND s.revoked_at IS NULL
               AND s.expires_at>now() AND u.account_state='active'
             FOR UPDATE OF u FOR SHARE OF s",
        )
        .bind(session_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        let (reporter, adult) = viewer.ok_or(BookShareError::Forbidden)?;
        // Lock the live grant until commit, just like the S016 share path.
        // Revocation cannot race past a protected Book report submission.
        let pro_adult_grant: Option<i64> = sqlx::query_scalar(
            "SELECT user_id FROM user_level_grants
             WHERE user_id=$1 AND level='pro_adult' AND is_active FOR SHARE",
        )
        .bind(reporter)
        .fetch_optional(&mut *tx)
        .await?;
        if !allowed(&book_level, adult, pro_adult_grant.is_some()) {
            return Err(BookShareError::NotFound);
        }
        if let Some(comment_id) = comment_id {
            // Hold the visible comment until the complaint is committed.
            // Hidden, deleted and chapter-edition comments cannot be reported.
            let found: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM discussion_comments
                 WHERE id=$1 AND book_id=$2 AND chapter_id IS NULL
                   AND version_id IS NULL AND visibility='visible' FOR SHARE",
            )
            .bind(comment_id)
            .bind(book_id)
            .fetch_optional(&mut *tx)
            .await?;
            if found.is_none() {
                return Err(BookShareError::NotFound);
            }
        }
        let old: Option<(i64, i64, Option<i64>, String, String)> = sqlx::query_as(
            "SELECT id,book_id,comment_id,reason,details FROM social_book_content_reports
             WHERE reporter_user_id=$1 AND command_key=$2",
        )
        .bind(reporter)
        .bind(command_key)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((id, previous_book, previous_comment, previous_reason, previous_details)) = old
        {
            if previous_book != book_id
                || previous_comment != comment_id
                || previous_reason != reason
                || previous_details != details
            {
                return Err(BookShareError::Conflict);
            }
            tx.commit().await?;
            return Ok(id);
        }
        let pending: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM social_book_content_reports
             WHERE reporter_user_id=$1 AND book_id=$2
               AND comment_id IS NOT DISTINCT FROM $3 AND state='pending')",
        )
        .bind(reporter)
        .bind(book_id)
        .bind(comment_id)
        .fetch_one(&mut *tx)
        .await?;
        if pending {
            return Err(BookShareError::Conflict);
        }
        let daily: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::BIGINT FROM social_book_content_reports
             WHERE reporter_user_id=$1
               AND reported_at>clock_timestamp()-INTERVAL '1 day'",
        )
        .bind(reporter)
        .fetch_one(&mut *tx)
        .await?;
        if daily >= 10 {
            return Err(BookShareError::RateLimited);
        }
        let report: i64 = sqlx::query_scalar(
            "INSERT INTO social_book_content_reports(
               book_id,comment_id,reporter_user_id,reason,details,command_key)
             VALUES($1,$2,$3,$4,$5,$6) RETURNING id",
        )
        .bind(book_id)
        .bind(comment_id)
        .bind(reporter)
        .bind(reason)
        .bind(details)
        .bind(command_key)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(report)
    }

    /// Do not include Book titles, synopsis or reader bodies in a complaint queue.
    pub async fn pending(
        &self,
        session_digest: &[u8; 32],
        limit: u16,
    ) -> Result<Vec<PendingBookContentReport>, BookShareError> {
        if !(1..=50).contains(&limit) {
            return Err(BookShareError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        administrator(&mut tx, session_digest).await?;
        let rows: Vec<(i64, i64, Option<i64>, i64, String, String)> = sqlx::query_as(
            "SELECT id,book_id,comment_id,reporter_user_id,reason,details
             FROM social_book_content_reports WHERE state='pending'
             ORDER BY reported_at,id LIMIT $1",
        )
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .map(|(id, book_id, comment_id, reporter_id, reason, details)| {
                PendingBookContentReport {
                    id,
                    book_id,
                    comment_id,
                    reporter_id,
                    reason,
                    details,
                }
            })
            .collect())
    }

    /// Exactly one immutable decision. Concurrent reviewers are serialized by
    /// the report row; a failed audit insert always rolls back the decision.
    pub async fn decide(
        &self,
        session_digest: &[u8; 32],
        report_id: i64,
        decision: &str,
        note: &str,
    ) -> Result<(), BookShareError> {
        let note = note.trim();
        if report_id <= 0
            || !matches!(decision, "reviewed" | "dismissed" | "hide_comment")
            || note.is_empty()
            || note.chars().count() > 500
            || note.contains('\0')
        {
            return Err(BookShareError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let reviewer = administrator(&mut tx, session_digest).await?;
        let report: Option<(String, i64, Option<i64>)> = sqlx::query_as(
            "SELECT state,book_id,comment_id FROM social_book_content_reports
             WHERE id=$1 FOR UPDATE",
        )
        .bind(report_id)
        .fetch_optional(&mut *tx)
        .await?;
        let (state, book_id, comment_id) = report.ok_or(BookShareError::NotFound)?;
        if state != "pending" {
            return Err(BookShareError::Conflict);
        }
        let hide_comment = decision == "hide_comment";
        if hide_comment {
            let comment_id = comment_id.ok_or(BookShareError::Invalid)?;
            // Explicit administrator action, never automatic on report.
            // Lock the exact visible public-Book comment to prevent stale sanctions.
            let found: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM discussion_comments WHERE id=$1 AND book_id=$2
                   AND chapter_id IS NULL AND version_id IS NULL
                   AND visibility='visible' FOR UPDATE",
            )
            .bind(comment_id)
            .bind(book_id)
            .fetch_optional(&mut *tx)
            .await?;
            if found.is_none() {
                return Err(BookShareError::Conflict);
            }
            let changed = sqlx::query(
                "UPDATE discussion_comments SET visibility='hidden'
                 WHERE id=$1 AND book_id=$2 AND chapter_id IS NULL
                   AND version_id IS NULL AND visibility='visible'",
            )
            .bind(comment_id)
            .bind(book_id)
            .execute(&mut *tx)
            .await?;
            if changed.rows_affected() != 1 {
                return Err(BookShareError::Conflict);
            }
        }
        let state = if decision == "dismissed" {
            "dismissed"
        } else {
            "reviewed"
        };
        let action = if hide_comment {
            "hide_book_comment"
        } else {
            "none"
        };
        sqlx::query(
            "INSERT INTO social_book_content_review_events
             (report_id,reviewer_user_id,decision,note,moderation_action)
             VALUES($1,$2,$3,$4,$5)",
        )
        .bind(report_id)
        .bind(reviewer)
        .bind(state)
        .bind(note)
        .bind(action)
        .execute(&mut *tx)
        .await?;
        let changed = sqlx::query(
            "UPDATE social_book_content_reports SET state=$2
             WHERE id=$1 AND state='pending'",
        )
        .bind(report_id)
        .bind(state)
        .execute(&mut *tx)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(BookShareError::Conflict);
        }
        tx.commit().await?;
        Ok(())
    }
}
