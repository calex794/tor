//! BR-060: owner-only ordinary Book deletion, never physical/privileged erasure.
use super::PgAuthoring;
use crate::services::authoring::AuthorError;
use sqlx::Row;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrdinaryDeletionView {
    pub title: String,
    pub status: String,
    pub revision: i64,
}

impl PgAuthoring {
    /// Metadata is owner scoped; active translators and book moderators may not
    /// initiate or complete deletion, regardless of their edit privileges.
    pub async fn ordinary_deletion_view(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<OrdinaryDeletionView>, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT b.title,b.lifecycle_status,b.lifecycle_revision
             FROM books b JOIN users u ON u.id=$1 AND u.account_state='active'
             WHERE b.id=$2 AND b.owner_user_id=$1
               AND b.visibility_status='active'
               AND NOT b.admin_hold AND NOT b.is_deleted",
        )
        .bind(actor)
        .bind(book_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        row.map(|r| {
            Ok(OrdinaryDeletionView {
                title: r.try_get("title").map_err(|_| AuthorError::Unavailable)?,
                status: r
                    .try_get("lifecycle_status")
                    .map_err(|_| AuthorError::Unavailable)?,
                revision: r
                    .try_get("lifecycle_revision")
                    .map_err(|_| AuthorError::Unavailable)?,
            })
        })
        .transpose()
    }

    /// Called ONLY after the web password worker has verified the submitted
    /// password against expected_password_hash. The DB repeats identity,
    /// password-hash and active-session checks within this exact mutation.
    /// The first step deindexes pending removal; the second finishes it.
    pub async fn advance_ordinary_deletion(
        &self,
        actor: i64,
        book_id: i64,
        expected_revision: i64,
        action: &str,
        session_digest: &[u8; 32],
        expected_password_hash: &str,
    ) -> Result<i64, AuthorError> {
        if actor <= 0
            || book_id <= 0
            || expected_revision <= 0
            || !matches!(action, "start" | "complete")
            || !expected_password_hash.starts_with("$argon2id$")
        {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        // Canonical owner, credential and session rows stay locked during
        // the Book transition; revoked/rotated sessions cannot win between
        // password check and committing deletion.
        let row = sqlx::query(
            "SELECT b.lifecycle_status,b.lifecycle_revision
             FROM books b
             JOIN users u ON u.id=$1 AND u.account_state='active'
                 AND u.password_hash=$4
             JOIN sessions s ON s.user_id=u.id AND s.token_hash=$3::bytea
                 AND s.revoked_at IS NULL AND s.expires_at>now()
             WHERE b.id=$2 AND b.owner_user_id=$1
               AND b.visibility_status='active'
               AND NOT b.admin_hold AND NOT b.is_deleted
             FOR UPDATE OF b FOR SHARE OF u,s",
        )
        .bind(actor)
        .bind(book_id)
        .bind(session_digest.as_slice())
        .bind(expected_password_hash)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?
        .ok_or(AuthorError::Missing)?;
        let before: String = row
            .try_get("lifecycle_status")
            .map_err(|_| AuthorError::Unavailable)?;
        let revision: i64 = row
            .try_get("lifecycle_revision")
            .map_err(|_| AuthorError::Unavailable)?;
        if revision != expected_revision {
            return Err(AuthorError::Conflict);
        }
        let next_state = match (action, before.as_str()) {
            ("start", "in_progress" | "completed" | "abandoned" | "on_hiatus") => {
                "pending_deletion"
            }
            ("complete", "pending_deletion") => "ordinary_deleted",
            _ => return Err(AuthorError::Conflict),
        };
        let next_revision = revision.checked_add(1).ok_or(AuthorError::Conflict)?;
        // Keep chapter records, immutable free version pins, paid purchase facts,
        // and reader progress intact. Draft visibility removes all public paths
        // without misusing moderation or hard/legal deletion flags.
        sqlx::query(
            "UPDATE books SET lifecycle_status=$2, lifecycle_revision=$3,
                visibility='draft',
                ordinary_deleted_at=CASE WHEN $2='ordinary_deleted'
                    THEN clock_timestamp() ELSE NULL END
             WHERE id=$1",
        )
        .bind(book_id)
        .bind(next_state)
        .bind(next_revision)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        sqlx::query(
            "INSERT INTO book_ordinary_deletion_events
             (book_id,actor_user_id,action,previous_status,next_status,
              previous_revision,next_revision)
             VALUES($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(book_id)
        .bind(actor)
        .bind(action)
        .bind(&before)
        .bind(next_state)
        .bind(revision)
        .bind(next_revision)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(next_revision)
    }
}
