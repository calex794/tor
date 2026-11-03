//! Historical URL lookups are provider neutral and read only.
//! No private-book title or URL is ever returned to a registering user.
use crate::services::authoring::AuthorError;
use sqlx::{Postgres, Transaction};

pub(super) async fn source_url_exists(
    tx: &mut Transaction<'_, Postgres>,
    target_language: &str,
    canonical_urls: &[String],
    excluded_book_id: Option<i64>,
) -> Result<bool, AuthorError> {
    if canonical_urls.is_empty() {
        return Ok(false);
    }
    let found: Option<i64> = sqlx::query_scalar(
        "SELECT b.id FROM books AS b
         WHERE b.target_language=$1 AND NOT b.is_deleted
           AND ($3::bigint IS NULL OR b.id<>$3)
           AND b.source_url <> ''
           AND book_registration_normalized_source_url(b.source_url)
               = ANY($2::text[])
         LIMIT 1",
    )
    .bind(target_language)
    .bind(canonical_urls)
    .bind(excluded_book_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    if found.is_some() {
        return Ok(true);
    }
    let extra: Option<i64> = sqlx::query_scalar(
        "SELECT l.book_id FROM book_registration_links AS l
         JOIN books AS b ON b.id=l.book_id
         WHERE l.target_language=$1 AND NOT b.is_deleted
           AND ($3::bigint IS NULL OR l.book_id<>$3)
           AND book_registration_normalized_source_url(l.source_url)
               = ANY($2::text[])
         LIMIT 1",
    )
    .bind(target_language)
    .bind(canonical_urls)
    .bind(excluded_book_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    Ok(extra.is_some())
}
