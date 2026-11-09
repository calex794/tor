//! S049: buyer-independent complaints on an actually published free E edition.
//! No body, selected quote, private chapter, or storage credential is copied.
#![forbid(unsafe_code)]
use super::book_shares::BookShareError;
use sqlx::{PgPool, Postgres, Transaction};

#[derive(Clone)]
pub struct PgFreeCommentReports {
    pool: PgPool,
}

fn valid_key(key: &str) -> bool {
    key.len() == 43
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

async fn eligible(
    tx: &mut Transaction<'_, Postgres>,
    digest: &[u8; 32],
    chapter: i64,
    version: i64,
) -> Result<i64, BookShareError> {
    // Match E's exact-current-publication guard before considering a report.
    let level: Option<String> = sqlx::query_scalar(
        "SELECT b.access_level FROM chapters ch
         JOIN books b ON b.id=ch.book_id
         JOIN chapter_publications p ON p.chapter_id=ch.id AND p.version_id=$2
         JOIN chapter_versions v ON v.chapter_id=ch.id AND v.id=p.version_id
         WHERE ch.id=$1 AND b.visibility='public' AND b.visibility_status='active'
           AND NOT b.admin_hold AND NOT b.is_deleted
           AND ch.visibility='public' AND ch.access_level='free'
           AND NOT ch.admin_hold AND NOT ch.is_deleted
           AND (ch.reader_released_at IS NULL OR ch.reader_released_at<=now())
         FOR SHARE OF b,ch,p,v",
    )
    .bind(chapter)
    .bind(version)
    .fetch_optional(&mut **tx)
    .await?;
    let level = level.ok_or(BookShareError::NotFound)?;
    let viewer: Option<(i64, bool)> = sqlx::query_as(
        "SELECT u.id,u.adult_enabled FROM users u
         JOIN sessions s ON s.user_id=u.id
         WHERE s.token_hash=$1 AND s.revoked_at IS NULL
           AND s.expires_at>now() AND u.account_state='active'
         FOR UPDATE OF u FOR SHARE OF s",
    )
    .bind(digest.as_slice())
    .fetch_optional(&mut **tx)
    .await?;
    let (actor, adult) = viewer.ok_or(BookShareError::Forbidden)?;
    // An active grant is required and held until commit for pro_adult books.
    let pro: Option<i64> = if level == "pro_adult" {
        sqlx::query_scalar(
            "SELECT user_id FROM user_level_grants
             WHERE user_id=$1 AND level='pro_adult' AND is_active FOR SHARE",
        )
        .bind(actor)
        .fetch_optional(&mut **tx)
        .await?
    } else {
        None
    };
    if !match level.as_str() {
        "basic" => true,
        "adult" => adult,
        "pro_adult" => pro.is_some(),
        _ => false,
    } {
        return Err(BookShareError::NotFound);
    }
    Ok(actor)
}

async fn visible(
    tx: &mut Transaction<'_, Postgres>,
    chapter: i64,
    version: i64,
    comment: i64,
) -> Result<(), BookShareError> {
    let target: Option<i64> = sqlx::query_scalar(
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
    .fetch_optional(&mut **tx)
    .await?;
    if target.is_none() {
        return Err(BookShareError::NotFound);
    }
    Ok(())
}

impl PgFreeCommentReports {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Read-only form check: no E or chapter body returned, and no leaked age
    /// metadata on 404. Submission performs the same tests under locks.
    pub async fn reportable(
        &self,
        digest: &[u8; 32],
        chapter: i64,
        version: i64,
        comment: i64,
    ) -> Result<(), BookShareError> {
        if chapter <= 0 || version <= 0 || comment <= 0 {
            return Err(BookShareError::NotFound);
        }
        let mut tx = self.pool.begin().await?;
        eligible(&mut tx, digest, chapter, version).await?;
        visible(&mut tx, chapter, version, comment).await?;
        tx.commit().await?;
        Ok(())
    }

    /// Exactly-once intent per actor and target. The reporter's locked account
    /// row serializes the quota and pending check against concurrent submissions.
    pub async fn submit(
        &self,
        digest: &[u8; 32],
        target: (i64, i64, i64),
        reason: &str,
        details: &str,
        command_key: &str,
    ) -> Result<i64, BookShareError> {
        let (chapter, version, comment) = target;
        let details = details.trim();
        if chapter <= 0
            || version <= 0
            || comment <= 0
            || !matches!(reason, "spam" | "abuse" | "misleading" | "other")
            || details.chars().count() > 500
            || details.contains('\0')
            || !valid_key(command_key)
        {
            return Err(BookShareError::Invalid);
        }
        let mut tx = self.pool.begin().await?;
        let actor = eligible(&mut tx, digest, chapter, version).await?;
        visible(&mut tx, chapter, version, comment).await?;
        let previous: Option<(i64, i64, i64, i64, String, String)> = sqlx::query_as(
            "SELECT id,chapter_id,version_id,comment_id,reason,details
             FROM social_free_comment_reports
             WHERE reporter_user_id=$1 AND command_key=$2",
        )
        .bind(actor)
        .bind(command_key)
        .fetch_optional(&mut *tx)
        .await?;
        if let Some((id, ch, ver, cm, old_reason, old_details)) = previous {
            if (ch, ver, cm) != target || old_reason != reason || old_details != details {
                return Err(BookShareError::Conflict);
            }
            tx.commit().await?;
            return Ok(id);
        }
        let pending: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM social_free_comment_reports
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
            "SELECT COUNT(*)::BIGINT FROM social_free_comment_reports
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
            "INSERT INTO social_free_comment_reports
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
