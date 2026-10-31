//! H-09/S023: all-or-nothing chapter import, optionally finalized with its queue row.
use super::PgAuthoring;
use crate::services::{
    authoring::{valid_command_key, AuthorError},
    chapter_file_import::{fingerprint, ImportedChapter, MAX_IMPORT_CHAPTERS},
};
use sha2::{Digest, Sha256};
use sqlx::{PgConnection, Row};
use std::collections::HashSet;

fn validate_batch(
    actor: i64,
    book_id: i64,
    command_key: &str,
    chapters: &[ImportedChapter],
) -> Result<(), AuthorError> {
    if actor <= 0
        || book_id <= 0
        || !valid_command_key(command_key)
        || chapters.is_empty()
        || chapters.len() > MAX_IMPORT_CHAPTERS
    {
        return Err(AuthorError::Invalid);
    }
    let mut seen = HashSet::new();
    for chapter in chapters {
        if !(1..=1_000_000).contains(&chapter.number)
            || !seen.insert(chapter.number)
            || !(1..=200).contains(&chapter.title.chars().count())
            || chapter.title.chars().any(char::is_control)
            || !(1..=50_000).contains(&chapter.body.chars().count())
            || chapter.body.contains('\0')
        {
            return Err(AuthorError::Invalid);
        }
    }
    Ok(())
}

/// Caller owns the transaction. This is the only code path that creates the batch
/// tombstone and every chapter/version/publication pointer.
async fn import_batch_in_transaction(
    connection: &mut PgConnection,
    actor: i64,
    book_id: i64,
    command_key: &str,
    chapters: &[ImportedChapter],
) -> Result<usize, AuthorError> {
    let digest = fingerprint(book_id, chapters);
    // A single book lock excludes concurrent publication and other writers.
    // A missing or unowned book is identically hidden from this caller.
    let visibility: Option<String> = sqlx::query_scalar(
        "SELECT visibility FROM books
         WHERE id=$1 AND owner_user_id=$2 AND NOT admin_hold AND NOT is_deleted
         FOR UPDATE",
    )
    .bind(book_id)
    .bind(actor)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    let visibility = visibility.ok_or(AuthorError::Missing)?;

    let old = sqlx::query(
        "SELECT book_id,content_sha256,chapter_count
         FROM chapter_import_batches
         WHERE user_id=$1 AND command_key=$2 FOR UPDATE",
    )
    .bind(actor)
    .bind(command_key)
    .fetch_optional(&mut *connection)
    .await
    .map_err(|_| AuthorError::Unavailable)?;
    if let Some(old) = old {
        let old_book: i64 = old
            .try_get("book_id")
            .map_err(|_| AuthorError::Unavailable)?;
        let old_hash: Vec<u8> = old
            .try_get("content_sha256")
            .map_err(|_| AuthorError::Unavailable)?;
        let count: i32 = old
            .try_get("chapter_count")
            .map_err(|_| AuthorError::Unavailable)?;
        if old_book != book_id
            || old_hash.as_slice() != digest.as_slice()
            || usize::try_from(count).ok() != Some(chapters.len())
        {
            return Err(AuthorError::Conflict);
        }
        return Ok(chapters.len());
    }
    if visibility != "draft" {
        // In particular, never auto-release a hundred new chapters on a public book.
        return Err(AuthorError::Conflict);
    }

    sqlx::query(
        "INSERT INTO chapter_import_batches
         (user_id,command_key,book_id,content_sha256,chapter_count)
         VALUES ($1,$2,$3,$4,$5)",
    )
    .bind(actor)
    .bind(command_key)
    .bind(book_id)
    .bind(digest.as_slice())
    .bind(i32::try_from(chapters.len()).map_err(|_| AuthorError::Invalid)?)
    .execute(&mut *connection)
    .await
    .map_err(|_| AuthorError::Unavailable)?;

    for chapter in chapters {
        let chapter_id: i64 = sqlx::query_scalar(
            "INSERT INTO chapters
             (book_id,chapter_no,title,visibility,access_level,price_amount,edit_revision)
             VALUES ($1,$2,$3,'public','free',0,1) RETURNING id",
        )
        .bind(book_id)
        .bind(chapter.number)
        .bind(&chapter.title)
        .fetch_one(&mut *connection)
        .await
        .map_err(|error| match error {
            sqlx::Error::Database(ref db) if db.code().as_deref() == Some("23505") => {
                AuthorError::Conflict
            }
            _ => AuthorError::Unavailable,
        })?;
        let body_sha: [u8; 32] = Sha256::digest(chapter.body.as_bytes()).into();
        let version_id: i64 = sqlx::query_scalar(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
             VALUES ($1,1,$2,$3) RETURNING id",
        )
        .bind(chapter_id)
        .bind(&chapter.body)
        .bind(body_sha.as_slice())
        .fetch_one(&mut *connection)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        sqlx::query(
            "INSERT INTO chapter_publications(chapter_id,version_id)
             VALUES ($1,$2)",
        )
        .bind(chapter_id)
        .bind(version_id)
        .execute(&mut *connection)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
    }
    Ok(chapters.len())
}

impl PgAuthoring {
    pub async fn import_draft_chapter_batch(
        &self,
        actor: i64,
        book_id: i64,
        command_key: &str,
        chapters: &[ImportedChapter],
    ) -> Result<usize, AuthorError> {
        validate_batch(actor, book_id, command_key, chapters)?;
        let mut tx = self.pool.begin().await.map_err(|_| AuthorError::Unavailable)?;
        let saved =
            import_batch_in_transaction(&mut tx, actor, book_id, command_key, chapters).await?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(saved)
    }

    /// G's durable job fence, B's batch command, all chapter versions and
    /// succeeded state share ONE transaction. The job row is locked first:
    /// cancellation either wins before this transaction (zero chapters) or
    /// waits and observes succeeded (no false cancellation).
    pub async fn complete_chapter_import_job(
        &self,
        job_id: i64,
        epoch: i64,
        chapters: &[ImportedChapter],
    ) -> Result<usize, AuthorError> {
        if job_id <= 0 || epoch <= 0 {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(|_| AuthorError::Unavailable)?;
        let row = sqlx::query(
            "SELECT status,lease_epoch,actor_user_id,book_id,command_key,
                    approved_fingerprint,expected_chapter_count,
                    COALESCE(locked_until > now(), FALSE) AS lease_valid
             FROM chapter_import_jobs WHERE id=$1 FOR UPDATE",
        )
        .bind(job_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?
        .ok_or(AuthorError::Conflict)?;
        let status: String = row.try_get("status").map_err(|_| AuthorError::Unavailable)?;
        let current_epoch: i64 = row
            .try_get("lease_epoch")
            .map_err(|_| AuthorError::Unavailable)?;
        let actor: i64 = row.try_get("actor_user_id").map_err(|_| AuthorError::Unavailable)?;
        let book: i64 = row.try_get("book_id").map_err(|_| AuthorError::Unavailable)?;
        let command_key: String = row
            .try_get("command_key")
            .map_err(|_| AuthorError::Unavailable)?;
        let approved: Vec<u8> = row
            .try_get("approved_fingerprint")
            .map_err(|_| AuthorError::Unavailable)?;
        let count: i32 = row
            .try_get("expected_chapter_count")
            .map_err(|_| AuthorError::Unavailable)?;
        let lease_valid: bool = row
            .try_get("lease_valid")
            .map_err(|_| AuthorError::Unavailable)?;
        validate_batch(actor, book, &command_key, chapters)?;
        let digest = fingerprint(book, chapters);
        if status != "running"
            || current_epoch != epoch
            || approved.as_slice() != digest.as_slice()
            || usize::try_from(count).ok() != Some(chapters.len())
            || !lease_valid
        {
            return Err(AuthorError::Conflict);
        }

        let saved =
            import_batch_in_transaction(&mut tx, actor, book, &command_key, chapters).await?;
        let updated = sqlx::query(
            "UPDATE chapter_import_jobs SET status='succeeded',locked_until=NULL,
                    saved_count=$3,last_error_code=NULL,updated_at=now(),finished_at=now()
             WHERE id=$1 AND lease_epoch=$2 AND status='running' AND locked_until>now()",
        )
        .bind(job_id)
        .bind(epoch)
        .bind(i32::try_from(saved).map_err(|_| AuthorError::Invalid)?)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if updated.rows_affected() != 1 {
            return Err(AuthorError::Conflict);
        }
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(saved)
    }
}
