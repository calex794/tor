//! H-09/S023: one explicit owner command creates all draft chapters or none.
//! Its transaction never makes a public book, public release event or paid grant.
use super::PgAuthoring;
use crate::services::{
    authoring::{valid_command_key, AuthorError},
    chapter_file_import::{fingerprint, ImportedChapter, MAX_IMPORT_CHAPTERS},
};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::collections::HashSet;

impl PgAuthoring {
    pub async fn import_draft_chapter_batch(
        &self,
        actor: i64,
        book_id: i64,
        command_key: &str,
        chapters: &[ImportedChapter],
    ) -> Result<usize, AuthorError> {
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
        let digest = fingerprint(book_id, chapters);
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;

        // A single book lock excludes concurrent publication and other writers.
        // A missing or unowned book is identically hidden from this caller.
        let visibility: Option<String> = sqlx::query_scalar(
            "SELECT visibility FROM books
             WHERE id=$1 AND owner_user_id=$2 AND NOT admin_hold AND NOT is_deleted
             FOR UPDATE",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
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
        .fetch_optional(&mut *tx)
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
        .execute(&mut *tx)
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
            .fetch_one(&mut *tx)
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
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
            sqlx::query(
                "INSERT INTO chapter_publications(chapter_id,version_id)
                 VALUES ($1,$2)",
            )
            .bind(chapter_id)
            .bind(version_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        }

        // Commit the command and every chapter together, or PostgreSQL rolls all back.
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(chapters.len())
    }
}
