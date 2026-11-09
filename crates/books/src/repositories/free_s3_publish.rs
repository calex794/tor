//! S349: a new free chapter reserves IDs without saving Markdown to PG.
//! G uploads and verifies S3 bytes before the B-owned attachment transaction.
//! This repository never calls S3 and never stores body_markdown.
use crate::{
    repositories::chapter_number,
    services::authoring::{valid_command_key, AuthorError, ChapterAccess, SaveChapter},
};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedFreeChapter {
    pub chapter_id: i64,
    pub version_id: i64,
    pub published: bool,
}

#[derive(Clone)]
pub struct PgFreeS3Publication {
    pool: PgPool,
}

fn write_error(error: sqlx::Error) -> AuthorError {
    if let sqlx::Error::Database(ref db) = error {
        if matches!(db.code().as_deref(), Some("23503" | "23505" | "23514")) {
            return AuthorError::Conflict;
        }
    }
    AuthorError::Unavailable
}

impl PgFreeS3Publication {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// No publication pointer or full text exists until S3 passes verification.
    /// Repeat of the same command returns the exact same ID pair.
    pub async fn prepare_new(
        &self,
        actor: i64,
        input: &SaveChapter,
    ) -> Result<PreparedFreeChapter, AuthorError> {
        if actor <= 0
            || input.book_id <= 0
            || input.chapter_id.is_some()
            || input.expected_revision != 0
            || input.number < 0
            || input.access != ChapterAccess::Free
            || input.price_amount != 0
            || !(1..=200).contains(&input.title.chars().count())
            || input.title.chars().any(char::is_control)
            || !(1..=50_000).contains(&input.body.chars().count())
            || input.body.contains('\0')
            || input.body.contains('\r')
            || !valid_command_key(&input.command_key)
        {
            return Err(AuthorError::Invalid);
        }
        let mut hasher = Sha256::new();
        for part in [
            "s349-new-free-s3-chapter-v1".to_owned(),
            input.book_id.to_string(),
            input.number.to_string(),
            input.title.clone(),
            input.body.clone(),
            input.access.as_str().to_owned(),
            input.price_amount.to_string(),
        ] {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
        let command_hash: [u8; 32] = hasher.finalize().into();
        let content_hash: [u8; 32] = Sha256::digest(input.body.as_bytes()).into();
        let mut tx = self.pool.begin().await.map_err(write_error)?;
        let visibility: Option<String> = sqlx::query_scalar(
            "SELECT visibility FROM books
             WHERE id=$1 AND owner_user_id=$2
               AND NOT admin_hold AND NOT is_deleted FOR UPDATE",
        )
        .bind(input.book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
        match visibility.as_deref() {
            None => return Err(AuthorError::Missing),
            Some("public" | "draft") => (),
            _ => return Err(AuthorError::Conflict),
        }

        let claimed: Option<i64> = sqlx::query_scalar(
            "INSERT INTO chapter_save_commands(user_id,command_key,payload_hash)
             VALUES($1,$2,$3)
             ON CONFLICT(user_id,command_key) DO NOTHING RETURNING user_id",
        )
        .bind(actor)
        .bind(&input.command_key)
        .bind(command_hash.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
        if claimed.is_none() {
            let row = sqlx::query(
                "SELECT payload_hash,chapter_id,version_id
                 FROM chapter_save_commands WHERE user_id=$1 AND command_key=$2
                 FOR UPDATE",
            )
            .bind(actor)
            .bind(&input.command_key)
            .fetch_one(&mut *tx)
            .await
            .map_err(write_error)?;
            let old_hash: Vec<u8> = row.try_get("payload_hash").map_err(write_error)?;
            if old_hash != command_hash {
                return Err(AuthorError::Conflict);
            }
            let chapter_id: i64 = row
                .try_get::<Option<i64>, _>("chapter_id")
                .map_err(write_error)?
                .ok_or(AuthorError::Conflict)?;
            let version_id: i64 = row
                .try_get::<Option<i64>, _>("version_id")
                .map_err(write_error)?
                .ok_or(AuthorError::Conflict)?;
            let published: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM chapter_publications
                    WHERE chapter_id=$1 AND version_id=$2
                )",
            )
            .bind(chapter_id)
            .bind(version_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(write_error)?;
            return Ok(PreparedFreeChapter {
                chapter_id,
                version_id,
                published,
            });
        }
        let number = chapter_number::assign(&mut tx, input.book_id, input.number).await?;
        let chapter_id: i64 = sqlx::query_scalar(
            "INSERT INTO chapters(
                book_id,chapter_no,title,visibility,access_level,price_amount,edit_revision
             ) VALUES ($1,$2,$3,'draft','free',0,1) RETURNING id",
        )
        .bind(input.book_id)
        .bind(number)
        .bind(&input.title)
        .fetch_one(&mut *tx)
        .await
        .map_err(write_error)?;
        let version_id: i64 = sqlx::query_scalar(
            "INSERT INTO chapter_versions(chapter_id,version_no,content_sha256)
             VALUES($1,1,$2) RETURNING id",
        )
        .bind(chapter_id)
        .bind(content_hash.as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(write_error)?;
        sqlx::query(
            "UPDATE chapter_save_commands
             SET chapter_id=$3,version_id=$4,saved_revision=1
             WHERE user_id=$1 AND command_key=$2",
        )
        .bind(actor)
        .bind(&input.command_key)
        .bind(chapter_id)
        .bind(version_id)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        tx.commit().await.map_err(write_error)?;
        Ok(PreparedFreeChapter {
            chapter_id,
            version_id,
            published: false,
        })
    }

    /// Called only AFTER a G verified PUT + bounded GET + SHA256 comparison.
    /// Commits the exact storage reference, public pointer and release fact
    /// atomically. On any failure the chapter remains hidden.
    pub async fn attach_verified_and_publish(
        &self,
        actor: i64,
        book_id: i64,
        edition: (i64, i64),
        metadata: (&str, [u8; 32], u64),
    ) -> Result<(), AuthorError> {
        let (chapter_id, version_id) = edition;
        let (private_bucket, content_hash, size_bytes) = metadata;
        if actor <= 0
            || book_id <= 0
            || chapter_id <= 0
            || version_id <= 0
            || !(1..=200_000).contains(&size_bytes)
            || content_hash == [0; 32]
            || private_bucket.starts_with("public")
        {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(write_error)?;
        let book_visibility: Option<String> = sqlx::query_scalar(
            "SELECT visibility FROM books WHERE id=$1 AND owner_user_id=$2
               AND visibility IN ('public','draft') AND NOT admin_hold AND NOT is_deleted
             FOR UPDATE",
        )
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
        let Some(book_visibility) = book_visibility else {
            return Err(AuthorError::Missing);
        };
        let chapter_visibility: Option<String> = sqlx::query_scalar(
            "SELECT visibility FROM chapters WHERE id=$1 AND book_id=$2
             AND access_level='free' AND price_amount=0
             AND NOT admin_hold AND NOT is_deleted FOR UPDATE",
        )
        .bind(chapter_id)
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
        let Some(chapter_visibility) = chapter_visibility else {
            return Err(AuthorError::Missing);
        };
        let version_hash: Option<Vec<u8>> = sqlx::query_scalar(
            "SELECT content_sha256 FROM chapter_versions
             WHERE chapter_id=$1 AND id=$2",
        )
        .bind(chapter_id)
        .bind(version_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?
        .flatten();
        if version_hash.as_deref() != Some(content_hash.as_slice()) {
            return Err(AuthorError::Conflict);
        }
        let current: Option<i64> = sqlx::query_scalar(
            "SELECT version_id FROM chapter_publications WHERE chapter_id=$1 FOR UPDATE",
        )
        .bind(chapter_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
        let key = format!("private/free-chapters/{chapter_id}/{version_id}.md");
        if let Some(published_version) = current {
            if published_version != version_id || chapter_visibility != "public" {
                return Err(AuthorError::Conflict);
            }
            let exact: bool = sqlx::query_scalar(
                "SELECT EXISTS (
                    SELECT 1 FROM free_chapter_storage_objects
                    WHERE chapter_id=$1 AND version_id=$2 AND bucket=$3
                      AND object_key=$4 AND sha256=$5 AND size_bytes=$6
                )",
            )
            .bind(chapter_id)
            .bind(version_id)
            .bind(private_bucket)
            .bind(&key)
            .bind(content_hash.as_slice())
            .bind(size_bytes as i64)
            .fetch_one(&mut *tx)
            .await
            .map_err(write_error)?;
            return if exact {
                Ok(())
            } else {
                Err(AuthorError::Conflict)
            };
        }
        if chapter_visibility != "draft" {
            return Err(AuthorError::Conflict);
        }
        sqlx::query(
            "INSERT INTO free_chapter_storage_objects
             (chapter_id,version_id,bucket,object_key,sha256,size_bytes)
             VALUES ($1,$2,$3,$4,$5,$6)",
        )
        .bind(chapter_id)
        .bind(version_id)
        .bind(private_bucket)
        .bind(&key)
        .bind(content_hash.as_slice())
        .bind(size_bytes as i64)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
            .bind(chapter_id)
            .bind(version_id)
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
        if book_visibility == "public" {
            sqlx::query(
                "INSERT INTO chapter_first_release_events
                 (chapter_id,version_id,creator_user_id) VALUES($1,$2,$3)",
            )
            .bind(chapter_id)
            .bind(version_id)
            .bind(actor)
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
        }
        sqlx::query(
            "UPDATE chapters SET visibility='public'
             WHERE id=$1 AND visibility='draft'",
        )
        .bind(chapter_id)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        tx.commit().await.map_err(write_error)?;
        Ok(())
    }
}
