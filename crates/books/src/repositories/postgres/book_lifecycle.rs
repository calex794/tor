//! S020: NEwBOO book lifecycle metadata, separate from editorial project status.
//! Publication, visibility, and paid reader entitlements are not modified here.
use super::{book_related::lock_manageable_member, PgAuthoring};
use crate::services::authoring::AuthorError;
use sqlx::Row;

pub struct ManagedBookLifecycle {
    pub title: String,
    pub status: String,
    pub revision: i64,
}

/// NEwBOO accepts pending_deletion only with additional identity step-up.
/// This deliberately narrow editor never initiates or cancels that workflow.
pub fn editable_lifecycle_status(value: &str) -> bool {
    matches!(
        value,
        "in_progress" | "completed" | "abandoned" | "on_hiatus"
    )
}

impl PgAuthoring {
    pub async fn manageable_lifecycle(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<ManagedBookLifecycle>, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT b.title, b.lifecycle_status, b.lifecycle_revision
             FROM books b WHERE b.id=$1 AND NOT b.is_deleted AND NOT b.admin_hold
               AND EXISTS (
                 SELECT 1 FROM book_members m WHERE m.book_id=b.id
                   AND m.user_id=$2 AND m.is_active
                   AND (m.role IN ('translator','moderator')
                        OR (m.role='owner' AND b.owner_user_id=$2))
               )",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        row.map(|row| {
            Ok(ManagedBookLifecycle {
                title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
                status: row
                    .try_get("lifecycle_status")
                    .map_err(|_| AuthorError::Unavailable)?,
                revision: row
                    .try_get("lifecycle_revision")
                    .map_err(|_| AuthorError::Unavailable)?,
            })
        })
        .transpose()
    }

    pub async fn update_lifecycle(
        &self,
        actor: i64,
        book_id: i64,
        expected_revision: i64,
        requested: &str,
    ) -> Result<i64, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Err(AuthorError::Missing);
        }
        if expected_revision <= 0 || !editable_lifecycle_status(requested) {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let current = sqlx::query(
            "SELECT lifecycle_status, lifecycle_revision
             FROM books WHERE id=$1 AND NOT is_deleted AND NOT admin_hold
             FOR UPDATE",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?
        .ok_or(AuthorError::Missing)?;
        // Book first, then live member FOR SHARE: revocation cannot race a write.
        lock_manageable_member(&mut tx, actor, book_id).await?;
        let current_status: String = current
            .try_get("lifecycle_status")
            .map_err(|_| AuthorError::Unavailable)?;
        if current_status == "pending_deletion" {
            return Err(AuthorError::Forbidden);
        }
        let revision: i64 = current
            .try_get("lifecycle_revision")
            .map_err(|_| AuthorError::Unavailable)?;
        if revision != expected_revision {
            return Err(AuthorError::Conflict);
        }
        let next = revision.checked_add(1).ok_or(AuthorError::Conflict)?;
        sqlx::query(
            "UPDATE books SET lifecycle_status=$2, lifecycle_revision=$3
             WHERE id=$1",
        )
        .bind(book_id)
        .bind(requested)
        .bind(next)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s020_lifecycle_is_not_publication_or_deletion_step_up() {
        for value in ["in_progress", "completed", "abandoned", "on_hiatus"] {
            assert!(editable_lifecycle_status(value));
        }
        for invalid in [
            "pending_deletion",
            "ongoing",
            "paused",
            "public",
            "hidden",
            "blocked",
            "adult",
            "",
            "completed ",
        ] {
            assert!(!editable_lifecycle_status(invalid));
        }
    }
}
