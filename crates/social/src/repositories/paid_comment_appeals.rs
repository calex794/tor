//! S049: independent author appeal of an explicit paid V1 comment sanction.
//! No one gets an extra paid-chapter grant, sealed text, or purchase facts.
#![forbid(unsafe_code)]
use super::book_shares::BookShareError;
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Clone)]
pub struct PgPaidCommentAppeals {
    pool: PgPool,
}

pub struct AuthorAppealRow {
    pub hide_event_id: i64,
    pub chapter_id: i64,
    pub version_id: i64,
    pub comment_id: i64,
    pub visibility: String,
    pub appeal_state: Option<String>,
}

pub struct PendingAppeal {
    pub id: i64,
    pub hide_event_id: i64,
    pub chapter_id: i64,
    pub version_id: i64,
    pub comment_id: i64,
    pub author_id: i64,
    pub original_reviewer_id: i64,
    pub reason: String,
}

fn valid_key(key: &str) -> bool {
    key.len() == 43
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

async fn actor(
    tx: &mut Transaction<'_, Postgres>,
    digest: &[u8; 32],
    filing: bool,
) -> Result<i64, BookShareError> {
    let query = if filing {
        "SELECT u.id FROM sessions s JOIN users u ON u.id=s.user_id
         WHERE s.token_hash=$1 AND s.revoked_at IS NULL AND s.expires_at>now()
           AND u.account_state='active' FOR UPDATE OF u FOR SHARE OF s"
    } else {
        "SELECT u.id FROM sessions s JOIN users u ON u.id=s.user_id
         WHERE s.token_hash=$1 AND s.revoked_at IS NULL AND s.expires_at>now()
           AND u.account_state='active' FOR SHARE OF s,u"
    };
    sqlx::query_scalar(query)
        .bind(digest.as_slice())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or(BookShareError::Forbidden)
}

async fn administrator(
    tx: &mut Transaction<'_, Postgres>,
    digest: &[u8; 32],
) -> Result<i64, BookShareError> {
    sqlx::query_scalar(
        "SELECT u.id FROM sessions s
         JOIN users u ON u.id=s.user_id
         JOIN account_roles r ON r.user_id=u.id
         WHERE s.token_hash=$1 AND s.revoked_at IS NULL AND s.expires_at>now()
           AND u.account_state='active' AND r.role='administrator' AND r.is_active
         FOR SHARE OF s,u,r",
    )
    .bind(digest.as_slice())
    .fetch_optional(&mut **tx)
    .await?
    .ok_or(BookShareError::Forbidden)
}

impl PgPaidCommentAppeals {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Author-only history of exact sanctioned E comments; does not query
    /// titles or chapter body. Revoked buyers retain a right to appeal their
    /// own comment without acquiring a read grant.
    pub async fn author_history(
        &self,
        digest: &[u8; 32],
        limit: u16,
    ) -> Result<Vec<AuthorAppealRow>, BookShareError> {
        if !(1..=50).contains(&limit) {
            return Err(BookShareError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let owner = actor(&mut tx, digest, false).await?;
        let rows: Vec<(i64, i64, i64, i64, String, Option<String>)> = sqlx::query_as(
            "SELECT e.id,r.chapter_id,r.version_id,c.id,c.visibility,a.state
             FROM social_paid_comment_review_events e
             JOIN social_paid_comment_reports r ON r.id=e.report_id
             JOIN discussion_comments c ON c.id=r.comment_id
             LEFT JOIN social_paid_comment_appeals a ON a.hide_event_id=e.id
             WHERE e.moderation_action='hide_paid_comment'
               AND c.author_user_id=$1 AND c.book_id IS NULL
               AND c.chapter_id=r.chapter_id AND c.version_id=r.version_id
               AND NOT EXISTS (
                 SELECT 1 FROM social_paid_comment_review_events newer
                 JOIN social_paid_comment_reports nr ON nr.id=newer.report_id
                 WHERE nr.comment_id=c.id AND newer.id>e.id
                   AND newer.moderation_action='hide_paid_comment')
             ORDER BY e.id DESC LIMIT $2",
        )
        .bind(owner)
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .map(
                |(hide_event_id, chapter_id, version_id, comment_id, visibility, appeal_state)| {
                    AuthorAppealRow {
                        hide_event_id,
                        chapter_id,
                        version_id,
                        comment_id,
                        visibility,
                        appeal_state,
                    }
                },
            )
            .collect())
    }

    /// One appeal per exact immutable hide event; owner account row serializes
    /// a five-per-30-days cap even for simultaneous submissions.
    pub async fn submit(
        &self,
        digest: &[u8; 32],
        hide_event_id: i64,
        reason: &str,
        command_key: &str,
    ) -> Result<i64, BookShareError> {
        let reason = reason.trim();
        if hide_event_id <= 0
            || !(20..=1000).contains(&reason.chars().count())
            || reason.contains('\0')
            || !valid_key(command_key)
        {
            return Err(BookShareError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let owner = actor(&mut tx, digest, true).await?;
        let target: Option<(i64, String)> = sqlx::query_as(
            "SELECT c.author_user_id,c.visibility
             FROM social_paid_comment_review_events e
             JOIN social_paid_comment_reports r ON r.id=e.report_id
             JOIN discussion_comments c ON c.id=r.comment_id
             WHERE e.id=$1 AND e.moderation_action='hide_paid_comment'
               AND c.book_id IS NULL AND c.chapter_id=r.chapter_id
               AND c.version_id=r.version_id
               AND NOT EXISTS (
                 SELECT 1 FROM social_paid_comment_review_events newer
                 JOIN social_paid_comment_reports nr ON nr.id=newer.report_id
                 WHERE nr.comment_id=c.id AND newer.id>e.id
                   AND newer.moderation_action='hide_paid_comment')
             FOR SHARE OF e,r,c",
        )
        .bind(hide_event_id)
        .fetch_optional(&mut *tx)
        .await?;
        let (author, visibility) = target.ok_or(BookShareError::NotFound)?;
        if owner != author {
            return Err(BookShareError::NotFound);
        }
        let prior: Option<(i64, i64, String)> = sqlx::query_as(
            "SELECT id,hide_event_id,reason FROM social_paid_comment_appeals
             WHERE author_user_id=$1 AND command_key=$2",
        )
        .bind(owner)
        .bind(command_key)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((id, old_event, old_reason)) = prior {
            if old_event != hide_event_id || old_reason != reason {
                return Err(BookShareError::Conflict);
            }
            tx.commit().await?;
            return Ok(id);
        }
        if visibility != "hidden" {
            return Err(BookShareError::Conflict);
        }
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM social_paid_comment_appeals
             WHERE hide_event_id=$1)",
        )
        .bind(hide_event_id)
        .fetch_one(&mut *tx)
        .await?;
        if exists {
            return Err(BookShareError::Conflict);
        }
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::BIGINT FROM social_paid_comment_appeals
             WHERE author_user_id=$1 AND filed_at>clock_timestamp()-INTERVAL '30 days'",
        )
        .bind(owner)
        .fetch_one(&mut *tx)
        .await?;
        if count >= 5 {
            return Err(BookShareError::RateLimited);
        }
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO social_paid_comment_appeals
             (hide_event_id,author_user_id,reason,command_key)
             VALUES($1,$2,$3,$4) RETURNING id",
        )
        .bind(hide_event_id)
        .bind(owner)
        .bind(reason)
        .bind(command_key)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }

    /// Administrator-only queue; metadata and author's reasons, no E body
    /// or chapter version plaintext is ever returned.
    pub async fn pending(
        &self,
        digest: &[u8; 32],
        limit: u16,
    ) -> Result<Vec<PendingAppeal>, BookShareError> {
        if !(1..=50).contains(&limit) {
            return Err(BookShareError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        administrator(&mut tx, digest).await?;
        let rows: Vec<(i64, i64, i64, i64, i64, i64, i64, String)> = sqlx::query_as(
            "SELECT a.id,a.hide_event_id,r.chapter_id,r.version_id,c.id,
                    a.author_user_id,e.reviewer_user_id,a.reason
             FROM social_paid_comment_appeals a
             JOIN social_paid_comment_review_events e ON e.id=a.hide_event_id
             JOIN social_paid_comment_reports r ON r.id=e.report_id
             JOIN discussion_comments c ON c.id=r.comment_id
             WHERE a.state='pending' AND e.moderation_action='hide_paid_comment'
               AND c.book_id IS NULL AND c.chapter_id=r.chapter_id
               AND c.version_id=r.version_id
             ORDER BY a.filed_at,a.id LIMIT $1",
        )
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(rows
            .into_iter()
            .map(
                |(
                    id,
                    hide_event_id,
                    chapter_id,
                    version_id,
                    comment_id,
                    author_id,
                    original_reviewer_id,
                    reason,
                )| PendingAppeal {
                    id,
                    hide_event_id,
                    chapter_id,
                    version_id,
                    comment_id,
                    author_id,
                    original_reviewer_id,
                    reason,
                },
            )
            .collect())
    }

    /// Only a different active administrator can restore the target comment.
    /// No path here changes chapter versions, purchase facts or reader rights.
    pub async fn decide(
        &self,
        digest: &[u8; 32],
        appeal_id: i64,
        decision: &str,
        note: &str,
    ) -> Result<(), BookShareError> {
        let note = note.trim();
        if appeal_id <= 0
            || !matches!(decision, "denied" | "restored")
            || note.is_empty()
            || note.chars().count() > 500
            || note.contains('\0')
        {
            return Err(BookShareError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let reviewer = administrator(&mut tx, digest).await?;
        let appeal: Option<(String, i64, i64)> = sqlx::query_as(
            "SELECT state,hide_event_id,author_user_id FROM social_paid_comment_appeals
             WHERE id=$1 FOR UPDATE",
        )
        .bind(appeal_id)
        .fetch_optional(&mut *tx)
        .await?;
        let (state, hide_event_id, author) = appeal.ok_or(BookShareError::NotFound)?;
        if state != "pending" {
            return Err(BookShareError::Conflict);
        }
        let target: Option<(i64, i64, i64, i64, i64, String)> = sqlx::query_as(
            "SELECT e.reviewer_user_id,r.chapter_id,r.version_id,
                    c.id,c.author_user_id,c.visibility
             FROM social_paid_comment_review_events e
             JOIN social_paid_comment_reports r ON r.id=e.report_id
             JOIN discussion_comments c ON c.id=r.comment_id
             WHERE e.id=$1 AND e.moderation_action='hide_paid_comment'
               AND c.book_id IS NULL AND c.chapter_id=r.chapter_id
               AND c.version_id=r.version_id
             FOR UPDATE OF c",
        )
        .bind(hide_event_id)
        .fetch_optional(&mut *tx)
        .await?;
        let (original_reviewer, chapter, version, comment, actual_author, visibility) =
            target.ok_or(BookShareError::Conflict)?;
        if reviewer == original_reviewer || reviewer == author || actual_author != author {
            return Err(BookShareError::Forbidden);
        }
        if decision == "restored" {
            if visibility != "hidden" {
                return Err(BookShareError::Conflict);
            }
            // A later sanction is independently authoritative. An appeal of
            // an old hide never restores an unrelated newer hidden state.
            let later_hide: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                   SELECT 1 FROM social_paid_comment_review_events e
                   JOIN social_paid_comment_reports r ON r.id=e.report_id
                   WHERE r.comment_id=$1 AND e.id>$2
                     AND e.moderation_action='hide_paid_comment')",
            )
            .bind(comment)
            .bind(hide_event_id)
            .fetch_one(&mut *tx)
            .await?;
            if later_hide {
                return Err(BookShareError::Conflict);
            }
            let changed = sqlx::query(
                "UPDATE discussion_comments SET visibility='visible'
                 WHERE id=$1 AND chapter_id=$2 AND version_id=$3
                   AND book_id IS NULL AND visibility='hidden'",
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
        sqlx::query(
            "INSERT INTO social_paid_comment_appeal_decisions
             (appeal_id,reviewer_user_id,decision,note) VALUES($1,$2,$3,$4)",
        )
        .bind(appeal_id)
        .bind(reviewer)
        .bind(decision)
        .bind(note)
        .execute(&mut *tx)
        .await?;
        let changed = sqlx::query(
            "UPDATE social_paid_comment_appeals SET state=$2
             WHERE id=$1 AND state='pending'",
        )
        .bind(appeal_id)
        .bind(decision)
        .execute(&mut *tx)
        .await?;
        if changed.rows_affected() != 1 {
            return Err(BookShareError::Conflict);
        }
        tx.commit().await?;
        Ok(())
    }
}
