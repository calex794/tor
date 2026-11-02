//! Read-only legacy source check during manual book creation.
use crate::services::authoring::AuthorError;
use sqlx::{Postgres, Transaction};

/// Strict provider-specific work IDs only; no fuzzy URL or title merging.
pub(super) async fn rulate_work_exists(
    tx: &mut Transaction<'_, Postgres>,
    target_language: &str,
    work_id: &str,
) -> Result<bool, AuthorError> {
    let row: Option<i32> = sqlx::query_scalar(
        "SELECT 1 FROM books AS b
         WHERE b.target_language=$1
           AND NOT b.is_deleted
           AND b.source_url <> ''
           AND book_registration_rulate_work_id(b.source_url)=$2
         LIMIT 1",
    )
    .bind(target_language)
    .bind(work_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    Ok(row.is_some())
}
