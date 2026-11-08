//! S020: active per-Book member edits of the public book annotation.
//! Book, member and description are checked in one transaction for writes.
use super::PgAuthoring;
use crate::services::authoring::{normalize_book_description, AuthorError};
use sqlx::Row;

pub struct ManagedBookDescription {
    pub title: String,
    pub description: String,
}

impl PgAuthoring {
    pub async fn manageable_book_description(
        &self,
        actor: i64,
        book_id: i64,
    ) -> Result<Option<ManagedBookDescription>, AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT b.title, b.description FROM books b
             WHERE b.id=$1 AND NOT b.admin_hold AND NOT b.is_deleted
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
            Ok(ManagedBookDescription {
                title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
                description: row
                    .try_get("description")
                    .map_err(|_| AuthorError::Unavailable)?,
            })
        })
        .transpose()
    }

    pub async fn update_book_description(
        &self,
        actor: i64,
        book_id: i64,
        raw: &str,
    ) -> Result<(), AuthorError> {
        if actor <= 0 || book_id <= 0 {
            return Err(AuthorError::Missing);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let book: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM books WHERE id=$1 AND NOT admin_hold AND NOT is_deleted
             FOR UPDATE",
        )
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if book.is_none() {
            return Err(AuthorError::Missing);
        }
        // Book -> member lock order matches B-09 revocation and S020 settings.
        super::book_related::lock_manageable_member(&mut tx, actor, book_id).await?;
        let value = normalize_book_description(raw)?;
        sqlx::query("UPDATE books SET description=$2 WHERE id=$1")
            .bind(book_id)
            .bind(value)
            .execute(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)
    }
}
