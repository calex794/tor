//! S049: authenticated complaints about comments on a purchased immutable V1.
//! Do not copy paid text, selected quotes, object keys or purchase secrets.
#![forbid(unsafe_code)]
use super::book_shares::BookShareError;
use sqlx::PgPool;

#[derive(Clone)]
pub struct PgPaidCommentReports {
    pool: PgPool,
}

impl PgPaidCommentReports {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// A command key is idempotent only for the exact target and same payload.
    /// The buyer, exact purchase, V1 and visible comment are all checked again
    /// while holding the active session and user locks in this transaction.
    pub async fn submit(
        &self,
        digest: &[u8; 32],
        actor: i64,
        target: (i64, i64, i64, i64),
        reason: &str,
        details: &str,
        command_key: &str,
    ) -> Result<i64, BookShareError> {
        let (purchase, chapter, version, comment) = target;
        let details = details.trim();
        if actor <= 0
            || purchase <= 0
            || chapter <= 0
            || version <= 0
            || comment <= 0
            || !matches!(reason, "spam" | "abuse" | "misleading" | "other")
            || details.chars().count() > 500
            || details.contains('\0')
            || command_key.len() != 43
            || !command_key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err(BookShareError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let live: Option<i64> = sqlx::query_scalar(
            "SELECT u.id FROM chapter_purchase_facts p
             JOIN library_purchased_chapters l ON l.purchase_id=p.id
               AND l.user_id=p.user_id AND l.chapter_id=p.chapter_id
               AND l.active_version_id=p.version_id
             JOIN chapters ch ON ch.id=p.chapter_id AND ch.access_level='paid'
             JOIN users u ON u.id=p.user_id AND u.account_state='active'
             JOIN sessions s ON s.user_id=u.id AND s.token_hash=$5
               AND s.revoked_at IS NULL AND s.expires_at>now()
             WHERE p.id=$1 AND p.user_id=$2 AND p.chapter_id=$3 AND p.version_id=$4
               AND tor_purchase_version_readable($2,p.id,p.chapter_id,p.version_id)
             FOR UPDATE OF u FOR SHARE OF p,ch,s",
        )
        .bind(purchase)
        .bind(actor)
        .bind(chapter)
        .bind(version)
        .bind(digest.as_slice())
        .fetch_optional(&mut *tx)
        .await?;
        if live != Some(actor) {
            return Err(BookShareError::Forbidden);
        }
        let visible: Option<i64> = sqlx::query_scalar(
            "SELECT c.id FROM discussion_comments c
             WHERE c.id=$1 AND c.chapter_id=$2 AND c.version_id=$3
               AND c.book_id IS NULL AND c.visibility='visible'
               AND (c.parent_id IS NULL OR EXISTS (
                    SELECT 1 FROM discussion_comments parent
                    WHERE parent.id=c.parent_id AND parent.chapter_id=$2
                      AND parent.version_id=$3 AND parent.visibility='visible'))
             FOR SHARE OF c",
        )
        .bind(comment)
        .bind(chapter)
        .bind(version)
        .fetch_optional(&mut *tx)
        .await?;
        if visible.is_none() {
            return Err(BookShareError::NotFound);
        }
        let previous: Option<(i64, i64, i64, i64, String, String)> = sqlx::query_as(
            "SELECT id,chapter_id,version_id,comment_id,reason,details
             FROM social_paid_comment_reports
             WHERE reporter_user_id=$1 AND command_key=$2",
        )
        .bind(actor)
        .bind(command_key)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((id, old_chapter, old_version, old_comment, old_reason, old_details)) = previous
        {
            if (old_chapter, old_version, old_comment) != (chapter, version, comment)
                || old_reason != reason
                || old_details != details
            {
                return Err(BookShareError::Conflict);
            }
            tx.commit().await?;
            return Ok(id);
        }
        let pending: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM social_paid_comment_reports
             WHERE reporter_user_id=$1 AND comment_id=$2 AND state='pending')",
        )
        .bind(actor)
        .bind(comment)
        .fetch_one(&mut *tx)
        .await?;
        if pending {
            return Err(BookShareError::Conflict);
        }
        let daily: i64 = sqlx::query_scalar(
            "SELECT COUNT(*)::BIGINT FROM social_paid_comment_reports
             WHERE reporter_user_id=$1
               AND reported_at>clock_timestamp()-INTERVAL '1 day'",
        )
        .bind(actor)
        .fetch_one(&mut *tx)
        .await?;
        if daily >= 10 {
            return Err(BookShareError::RateLimited);
        }
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO social_paid_comment_reports
             (chapter_id,version_id,comment_id,reporter_user_id,reason,details,command_key)
             VALUES($1,$2,$3,$4,$5,$6,$7) RETURNING id",
        )
        .bind(chapter)
        .bind(version)
        .bind(comment)
        .bind(actor)
        .bind(reason)
        .bind(details)
        .bind(command_key)
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(id)
    }
}
