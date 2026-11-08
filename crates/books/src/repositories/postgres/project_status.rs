//! S020: live scoped Book-member editorial progress, separate from publication and access.
use super::{book_related::lock_manageable_member, PgAuthoring};
use crate::services::authoring::AuthorError;
use sqlx::Row;

pub struct OwnedProjectStatus {
    pub title: String,
    pub status: String,
    pub revision: i64,
}

pub fn valid_project_status(value: &str) -> bool {
    matches!(value, "" | "ongoing" | "completed" | "paused" | "planned")
}

impl PgAuthoring {
    pub async fn manageable_project_status(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<OwnedProjectStatus>, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT b.title, b.project_status, b.project_status_revision
             FROM books b WHERE b.id=$1
               AND NOT b.is_deleted AND NOT b.admin_hold
               AND EXISTS (
                 SELECT 1 FROM book_members m
                 WHERE m.book_id=b.id AND m.user_id=$2 AND m.is_active
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
            Ok(OwnedProjectStatus {
                title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
                status: row
                    .try_get("project_status")
                    .map_err(|_| AuthorError::Unavailable)?,
                revision: row
                    .try_get("project_status_revision")
                    .map_err(|_| AuthorError::Unavailable)?,
            })
        })
        .transpose()
    }

    /// A stale tab never overwrites another status update. Neither visibility
    /// nor any purchase, revision or reader permission is modified.
    pub async fn update_project_status(
        &self,
        actor: i64,
        book_id: i64,
        expected_revision: i64,
        raw_status: &str,
    ) -> Result<i64, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Err(AuthorError::Missing);
        }
        if expected_revision <= 0 || !valid_project_status(raw_status) {
            return Err(AuthorError::Invalid);
        }
        // Lock the source Book before the live membership check. The row
        // share lock also prevents a concurrent role revoke during the write.
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let revision: Option<i64> = sqlx::query_scalar(
            "SELECT project_status_revision FROM books
             WHERE id=$1 AND NOT is_deleted AND NOT admin_hold
             FOR UPDATE",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let revision = revision.ok_or(AuthorError::Missing)?;
        lock_manageable_member(&mut tx, actor, book_id).await?;
        if revision != expected_revision {
            return Err(AuthorError::Conflict);
        }
        let next_revision = revision.checked_add(1).ok_or(AuthorError::Conflict)?;
        sqlx::query(
            "UPDATE books SET project_status=$2, project_status_revision=$3
             WHERE id=$1",
        )
        .bind(book_id)
        .bind(raw_status)
        .bind(next_revision)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(next_revision)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s020_project_status_is_not_publication_or_entitlement() {
        for value in ["", "ongoing", "completed", "paused", "planned"] {
            assert!(valid_project_status(value));
        }
        for invalid in [
            "public",
            "hidden",
            "paid",
            "blocked",
            "completed ",
            "deleted",
        ] {
            assert!(!valid_project_status(invalid));
        }
    }
}
