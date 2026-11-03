//! S020: author-only editorial progress, separate from publication and access.
use super::PgAuthoring;
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
    pub async fn owned_project_status(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<OwnedProjectStatus>, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT title, project_status, project_status_revision
             FROM books WHERE id=$1 AND owner_user_id=$2
               AND NOT is_deleted AND NOT admin_hold",
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
        let revision = sqlx::query_scalar::<_, i64>(
            "UPDATE books SET project_status=$4,
                    project_status_revision=project_status_revision+1
             WHERE id=$1 AND owner_user_id=$2
               AND project_status_revision=$3
               AND project_status_revision < 9223372036854775807
               AND NOT is_deleted AND NOT admin_hold
             RETURNING project_status_revision",
        )
        .bind(book_id)
        .bind(actor)
        .bind(expected_revision)
        .bind(raw_status)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if let Some(revision) = revision {
            return Ok(revision);
        }
        if self.owned_project_status(actor, book_id).await?.is_some() {
            Err(AuthorError::Conflict)
        } else {
            Err(AuthorError::Missing)
        }
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
