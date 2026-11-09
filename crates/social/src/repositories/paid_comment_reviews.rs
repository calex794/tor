//! S049: administrator-only, audited manual review of purchased-edition comment reports.
//! This service never fetches chapter body, object keys, purchase facts or paid grants.
#![forbid(unsafe_code)]
use super::book_shares::BookShareError;
use sqlx::{PgPool, Postgres, Row, Transaction};

#[derive(Clone)]
pub struct PgPaidCommentReviews {
    pool: PgPool,
}

pub struct PendingPaidCommentReport {
    pub id: i64,
    pub chapter_id: i64,
    pub version_id: i64,
    pub comment_id: i64,
    pub reporter_id: i64,
    pub reason: String,
    pub details: String,
    pub comment_body: String,
}

pub struct ReviewedPaidCommentReport {
    pub event_id: i64,
    pub report_id: i64,
    pub comment_id: i64,
    pub reviewer_id: i64,
    pub decision: String,
    pub moderation_action: String,
    pub note: String,
    pub decided_utc: String,
}

pub struct ReviewedPaidCommentPage {
    pub items: Vec<ReviewedPaidCommentReport>,
    pub next_cursor: Option<i64>,
}

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

impl PgPaidCommentReviews {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Only active site administrators see E comment bodies. Chapter plaintext
    /// is not joined, even for administrators.
    pub async fn pending(
        &self,
        session_digest: &[u8; 32],
        limit: u16,
    ) -> Result<Vec<PendingPaidCommentReport>, BookShareError> {
        if !(1..=50).contains(&limit) {
            return Err(BookShareError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        administrator(&mut tx, session_digest).await?;
        let rows = sqlx::query(
            "SELECT r.id,r.chapter_id,r.version_id,r.comment_id,
                    r.reporter_user_id,r.reason,r.details,c.body AS comment_body
             FROM social_paid_comment_reports r
             JOIN discussion_comments c ON c.id=r.comment_id
               AND c.chapter_id=r.chapter_id AND c.version_id=r.version_id
             WHERE r.state='pending'
             ORDER BY r.reported_at,r.id LIMIT $1",
        )
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await?;
        let items = rows
            .into_iter()
            .map(|row| {
                Ok(PendingPaidCommentReport {
                    id: row.try_get("id")?,
                    chapter_id: row.try_get("chapter_id")?,
                    version_id: row.try_get("version_id")?,
                    comment_id: row.try_get("comment_id")?,
                    reporter_id: row.try_get("reporter_user_id")?,
                    reason: row.try_get("reason")?,
                    details: row.try_get("details")?,
                    comment_body: row.try_get("comment_body")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        tx.commit().await?;
        Ok(items)
    }

    /// Exactly one explicit administrator decision. "hide_comment" hides
    /// ONLY the exact visible E comment, never the sealed chapter or any grants.
    /// Report, comment and live role locks span the immutable audit commit.
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
        let report: Option<(String, i64, i64, i64)> = sqlx::query_as(
            "SELECT state,chapter_id,version_id,comment_id
             FROM social_paid_comment_reports WHERE id=$1 FOR UPDATE",
        )
        .bind(report_id)
        .fetch_optional(&mut *tx)
        .await?;
        let (state, chapter, version, comment) =
            report.ok_or(BookShareError::NotFound)?;
        if state != "pending" {
            return Err(BookShareError::Conflict);
        }
        let hide = decision == "hide_comment";
        if hide {
            // Revalidate the exact, still-visible V1 E comment under lock.
            // Reject stale or forged report bindings and already hidden items.
            let found: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM discussion_comments c
                 WHERE c.id=$1 AND c.chapter_id=$2 AND c.version_id=$3
                   AND c.book_id IS NULL AND c.visibility='visible'
                   AND (c.parent_id IS NULL OR EXISTS (
                        SELECT 1 FROM discussion_comments parent
                        WHERE parent.id=c.parent_id AND parent.chapter_id=$2
                          AND parent.version_id=$3 AND parent.visibility='visible'))
                 FOR UPDATE OF c",
            )
            .bind(comment)
            .bind(chapter)
            .bind(version)
            .fetch_optional(&mut *tx)
            .await?;
            if found.is_none() {
                return Err(BookShareError::Conflict);
            }
            let changed = sqlx::query(
                "UPDATE discussion_comments SET visibility='hidden'
                 WHERE id=$1 AND chapter_id=$2 AND version_id=$3
                   AND book_id IS NULL AND visibility='visible'",
            )
            .bind(comment)
            .bind(chapter)
            .bind(version)
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
        let action = if hide { "hide_paid_comment" } else { "none" };
        sqlx::query(
            "INSERT INTO social_paid_comment_review_events
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
            "UPDATE social_paid_comment_reports SET state=$2
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

    /// Immutable, descending decision history; includes no report text or
    /// chapter content, and never exposes another actor's session.
    pub async fn history(
        &self,
        session_digest: &[u8; 32],
        before_id: Option<i64>,
        limit: u16,
    ) -> Result<ReviewedPaidCommentPage, BookShareError> {
        if !(1..=50).contains(&limit) || before_id.is_some_and(|id| id <= 0) {
            return Err(BookShareError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        administrator(&mut tx, session_digest).await?;
        let rows = sqlx::query(
            "SELECT e.id AS event_id,e.report_id,r.comment_id,
                    e.reviewer_user_id,e.decision,e.moderation_action,e.note,
                    to_char(e.decided_at AT TIME ZONE 'UTC',
                            'YYYY-MM-DD HH24:MI:SS') AS decided_utc
             FROM social_paid_comment_review_events e
             JOIN social_paid_comment_reports r ON r.id=e.report_id
             WHERE ($1::BIGINT IS NULL OR e.id<$1)
             ORDER BY e.id DESC LIMIT $2",
        )
        .bind(before_id)
        .bind(i64::from(limit) + 1)
        .fetch_all(&mut *tx)
        .await?;
        let has_more = rows.len() > usize::from(limit);
        let items = rows
            .into_iter()
            .take(usize::from(limit))
            .map(|row| {
                Ok(ReviewedPaidCommentReport {
                    event_id: row.try_get("event_id")?,
                    report_id: row.try_get("report_id")?,
                    comment_id: row.try_get("comment_id")?,
                    reviewer_id: row.try_get("reviewer_user_id")?,
                    decision: row.try_get("decision")?,
                    moderation_action: row.try_get("moderation_action")?,
                    note: row.try_get("note")?,
                    decided_utc: row.try_get("decided_utc")?,
                })
            })
            .collect::<Result<Vec<_>, sqlx::Error>>()?;
        tx.commit().await?;
        let next_cursor = has_more.then(|| items.last().map(|x| x.event_id)).flatten();
        Ok(ReviewedPaidCommentPage { items, next_cursor })
    }
}
