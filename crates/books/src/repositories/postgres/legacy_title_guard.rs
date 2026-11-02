//! Indexed, read-only check against legacy books without title claims.
//! The unique book_registration_titles constraint still resolves races among
//! new registrations; a legacy book is never rewritten or made public.
use crate::services::authoring::AuthorError;
use sqlx::{Postgres, Transaction};

pub(super) async fn exists(
    tx: &mut Transaction<'_, Postgres>,
    target_language: &str,
    normalized_titles: &[String],
) -> Result<bool, AuthorError> {
    let found: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM books AS b
         WHERE b.target_language = $1
           AND NOT b.is_deleted
           AND book_registration_legacy_title_key(b.title) = ANY($2::text[])
           AND NOT EXISTS (
               SELECT 1 FROM book_registration_titles AS claimed
               WHERE claimed.book_id = b.id
           )
         LIMIT 1",
    )
    .bind(target_language)
    .bind(normalized_titles)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    Ok(found.is_some())
}
