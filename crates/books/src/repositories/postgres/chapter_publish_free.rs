//! A normal Save to an already public book releases a free chapter in one
//! atomic owner-only transaction. The historical private/draft commands remain
//! separate for books that are not yet published.
use super::{chapter_outbox::enqueue_chapter_published, PgAuthoring};
use crate::services::authoring::{
    valid_command_key, AuthorError, ChapterAccess, SaveChapter, SavedChapter,
};
use sha2::{Digest, Sha256};
use sqlx::Row;

impl PgAuthoring {
    /// One user action means one published free chapter: immutable version,
    /// reader pointer and first-release event must commit together.
    pub async fn create_published_free_chapter(
        &self,
        actor: i64,
        input: &SaveChapter,
    ) -> Result<SavedChapter, AuthorError> {
        if actor <= 0
            || input.book_id <= 0
            || input.chapter_id.is_some()
            || input.expected_revision != 0
            || input.number <= 0
            || input.access != ChapterAccess::Free
            || input.price_amount != 0
            || !(1..=200).contains(&input.title.chars().count())
            || input.title.chars().any(char::is_control)
            || !(1..=50_000).contains(&input.body.chars().count())
            || input.body.contains('\0')
            || !valid_command_key(&input.command_key)
        {
            return Err(AuthorError::Invalid);
        }

        // Independent command namespace prevents an old, explicit pending
        // command from accidentally replaying as a public release.
        let mut hasher = Sha256::new();
        for field in [
            "public-book-immediate-free-save-v1".to_owned(),
            input.book_id.to_string(),
            input.number.to_string(),
            input.title.clone(),
            input.body.clone(),
            input.access.as_str().to_owned(),
            input.price_amount.to_string(),
        ] {
            hasher.update((field.len() as u64).to_be_bytes());
            hasher.update(field.as_bytes());
        }
        let payload_hash: [u8; 32] = hasher.finalize().into();
        let content_hash: [u8; 32] = Sha256::digest(input.body.as_bytes()).into();
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let visibility: Option<String> = sqlx::query_scalar(
            "SELECT visibility FROM books WHERE id=$1 AND owner_user_id=$2
             AND NOT admin_hold AND NOT is_deleted FOR UPDATE",
        )
        .bind(input.book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        match visibility.as_deref() {
            None => return Err(AuthorError::Missing),
            Some("public") => {}
            Some(_) => return Err(AuthorError::Conflict),
        }

        let claimed: Option<i64> = sqlx::query_scalar(
            "INSERT INTO chapter_save_commands (user_id,command_key,payload_hash)
             VALUES ($1,$2,$3) ON CONFLICT(user_id,command_key) DO NOTHING
             RETURNING user_id",
        )
        .bind(actor)
        .bind(&input.command_key)
        .bind(payload_hash.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if claimed.is_none() {
            let row = sqlx::query(
                "SELECT payload_hash,chapter_id,version_id,saved_revision
                 FROM chapter_save_commands WHERE user_id=$1 AND command_key=$2 FOR UPDATE",
            )
            .bind(actor)
            .bind(&input.command_key)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
            let old_hash: Vec<u8> = row
                .try_get("payload_hash")
                .map_err(|_| AuthorError::Unavailable)?;
            if old_hash.as_slice() != payload_hash.as_slice() {
                return Err(AuthorError::Conflict);
            }
            return Ok(SavedChapter {
                chapter_id: row
                    .try_get::<Option<i64>, _>("chapter_id")
                    .map_err(|_| AuthorError::Unavailable)?
                    .ok_or(AuthorError::Unavailable)?,
                version_id: row
                    .try_get::<Option<i64>, _>("version_id")
                    .map_err(|_| AuthorError::Unavailable)?
                    .ok_or(AuthorError::Unavailable)?,
                revision: row
                    .try_get::<Option<i64>, _>("saved_revision")
                    .map_err(|_| AuthorError::Unavailable)?
                    .ok_or(AuthorError::Unavailable)?,
            });
        }

        let chapter_id: i64 = sqlx::query_scalar(
            "INSERT INTO chapters
                 (book_id,chapter_no,title,visibility,access_level,price_amount,edit_revision)
             VALUES ($1,$2,$3,'public','free',0,1) RETURNING id",
        )
        .bind(input.book_id)
        .bind(input.number)
        .bind(&input.title)
        .fetch_one(&mut *tx)
        .await
        .map_err(|error| match error {
            sqlx::Error::Database(ref db) if db.code().as_deref() == Some("23505") => {
                AuthorError::Conflict
            }
            _ => AuthorError::Unavailable,
        })?;
        let version_id: i64 = sqlx::query_scalar(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
             VALUES ($1,1,$2,$3) RETURNING id",
        )
        .bind(chapter_id)
        .bind(&input.body)
        .bind(content_hash.as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
            .bind(chapter_id)
            .bind(version_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        sqlx::query(
            "INSERT INTO chapter_first_release_events(chapter_id,version_id)
             VALUES($1,$2)",
        )
        .bind(chapter_id)
        .bind(version_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        enqueue_chapter_published(&mut tx, input.book_id, chapter_id, version_id, true).await?;
        sqlx::query(
            "UPDATE chapter_save_commands SET chapter_id=$3,version_id=$4,saved_revision=1
             WHERE user_id=$1 AND command_key=$2",
        )
        .bind(actor)
        .bind(&input.command_key)
        .bind(chapter_id)
        .bind(version_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(SavedChapter {
            chapter_id,
            version_id,
            revision: 1,
        })
    }
}
