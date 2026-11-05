//! BR-060: owner-only ordinary Book deletion, never physical/privileged erasure.
use super::PgAuthoring;
use crate::services::authoring::AuthorError;
use sqlx::Row;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OrdinaryDeletionView {
    pub title: String,
    pub status: String,
    pub revision: i64,
    pub moderator_approved: bool,
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
            "SELECT b.title,
               CASE WHEN EXISTS (
                 SELECT 1 FROM book_ordinary_deletion_requests r
                 WHERE r.book_id=b.id AND r.request_revision=b.lifecycle_revision
                   AND NOT EXISTS (
                     SELECT 1 FROM book_ordinary_deletion_decisions d
                     WHERE d.book_id=r.book_id AND d.request_revision=r.request_revision
                   )
               ) THEN 'pending_deletion'
                    ELSE b.lifecycle_status END AS lifecycle_status,
               b.lifecycle_revision,
               EXISTS(
                 SELECT 1 FROM book_ordinary_deletion_decisions d
                 WHERE d.book_id=b.id AND d.request_revision=b.lifecycle_revision
                   AND d.decision='approve' AND b.lifecycle_status='pending_deletion'
               ) AS moderator_approved
             FROM books b JOIN users u ON u.id=$1 AND u.account_state='active'
             JOIN book_members m ON m.book_id=b.id AND m.user_id=$1
                 AND m.role='owner' AND m.is_active
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
                moderator_approved: r
                    .try_get("moderator_approved")
                    .map_err(|_| AuthorError::Unavailable)?,
            })
        })
        .transpose()
    }

    /// Called ONLY after the web password worker has verified the submitted
    /// password against expected_password_hash. The DB repeats identity,
    /// password-hash and active-session checks within this exact mutation.
    /// The owner's request creates a pending review without deindexing.
    /// Only a separate live site moderator may end public availability.
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
            || action != "start"
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
            "SELECT b.lifecycle_status,b.lifecycle_revision,b.visibility,
                    EXISTS (
                 SELECT 1 FROM book_ordinary_deletion_requests r
                 WHERE r.book_id=b.id AND r.request_revision=b.lifecycle_revision
                   AND NOT EXISTS (
                     SELECT 1 FROM book_ordinary_deletion_decisions d
                     WHERE d.book_id=r.book_id AND d.request_revision=r.request_revision
                   )
               ) AS pending_request
             FROM books b
             JOIN users u ON u.id=$1 AND u.account_state='active'
                 AND u.password_hash=$4
             JOIN sessions s ON s.user_id=u.id AND s.token_hash=$3::bytea
                 AND s.revoked_at IS NULL AND s.expires_at>now()
             JOIN book_members m ON m.book_id=b.id AND m.user_id=$1
                 AND m.role='owner' AND m.is_active
             WHERE b.id=$2 AND b.owner_user_id=$1
               AND b.visibility_status='active'
               AND NOT b.admin_hold AND NOT b.is_deleted
             FOR UPDATE OF b FOR SHARE OF u,s,m",
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
        let previous_visibility: String = row
            .try_get("visibility")
            .map_err(|_| AuthorError::Unavailable)?;
        let pending_request: bool = row
            .try_get("pending_request")
            .map_err(|_| AuthorError::Unavailable)?;
        if revision != expected_revision || pending_request {
            return Err(AuthorError::Conflict);
        }
        // Only a moderation decision changes Book lifecycle/public visibility.
        // The request increments its own exact Book revision for CAS, but the
        // previously approved book stays readable and purchasable until review.
        if !matches!(
            before.as_str(),
            "in_progress" | "completed" | "abandoned" | "on_hiatus"
        ) {
            return Err(AuthorError::Conflict);
        }
        let next_revision = revision.checked_add(1).ok_or(AuthorError::Conflict)?;
        sqlx::query("UPDATE books SET lifecycle_revision=$2 WHERE id=$1")
            .bind(book_id)
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
        .bind(&before)
        .bind(revision)
        .bind(next_revision)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if action == "start" {
            sqlx::query(
                "INSERT INTO book_ordinary_deletion_requests
                 (book_id,request_revision,owner_user_id,
                  previous_lifecycle_status,previous_visibility)
                 VALUES($1,$2,$3,$4,$5)",
            )
            .bind(book_id)
            .bind(next_revision)
            .bind(actor)
            .bind(&before)
            .bind(&previous_visibility)
            .execute(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        }
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(next_revision)
    }
}
