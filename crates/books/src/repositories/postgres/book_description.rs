//! Owner-scoped description edits for SEO-02b.
use super::PgAuthoring;
use crate::services::authoring::{normalize_book_description, AuthorError};

impl PgAuthoring {
    pub async fn owned_book_description(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<String>, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        sqlx::query_scalar(
            "SELECT description FROM books WHERE id=$1 AND owner_user_id=$2
             AND NOT admin_hold AND NOT is_deleted",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)
    }

    pub async fn update_book_description(
        &self,
        actor: i64,
        book_id: i64,
        raw: &str,
    ) -> Result<(), AuthorError> {
        if actor <= 0 {
            return Err(AuthorError::Forbidden);
        }
        if book_id <= 0 {
            return Err(AuthorError::Missing);
        }
        let value = normalize_book_description(raw)?;
        let result = sqlx::query(
            "UPDATE books SET description=$3 WHERE id=$1 AND owner_user_id=$2
             AND NOT admin_hold AND NOT is_deleted",
        )
        .bind(book_id)
        .bind(actor)
        .bind(value)
        .execute(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if result.rows_affected() == 1 {
            Ok(())
        } else {
            Err(AuthorError::Missing)
        }
    }
}
