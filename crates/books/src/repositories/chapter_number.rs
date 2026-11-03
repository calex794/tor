//! NEwBOO parity: allocate a chapter number while the caller holds the
//! authenticated owner book row FOR UPDATE in the same transaction.
//! The book lock serializes all writer paths and their auto-number choice.
use crate::services::authoring::AuthorError;
use sqlx::{Postgres, Transaction};

pub(crate) async fn assign(
    tx: &mut Transaction<'_, Postgres>,
    book_id: i64,
    requested: i32,
) -> Result<i32, AuthorError> {
    if requested < 0 {
        return Err(AuthorError::Invalid);
    }
    if requested > 0 {
        return Ok(requested);
    }
    // Existing TOR unique(book_id, chapter_no) covers deleted chapters too.
    let next: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(chapter_no::bigint),0)+1 FROM chapters WHERE book_id=$1",
    )
    .bind(book_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    i32::try_from(next).map_err(|_| AuthorError::Conflict)
}
