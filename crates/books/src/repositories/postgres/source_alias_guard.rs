//! Indexed read-only check against historical books with arbitrary source URLs.
//! New registrations also retain UNIQUE(source_identity,target_language).
use crate::services::authoring::AuthorError;
use sqlx::{Postgres, Transaction};

/// Only exact provider-neutral normalized URLs count as the same source.
/// Do not expose historical private books to the registering author.
pub(super) async fn source_url_exists(
    tx: &mut Transaction<'_, Postgres>,
    target_language: &str,
    canonical_url: &str,
) -> Result<bool, AuthorError> {
    let found: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM books AS b
         WHERE b.target_language=$1
           AND NOT b.is_deleted
           AND b.source_url <> ''
           AND book_registration_normalized_source_url(b.source_url)=$2
         LIMIT 1",
    )
    .bind(target_language)
    .bind(canonical_url)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    Ok(found.is_some())
}
