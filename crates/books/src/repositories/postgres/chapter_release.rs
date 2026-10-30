//! B-06: unpublished new chapters inside an already public book.
//! No reader-visible pointer is created until an explicit owner release.
use super::{chapter_outbox::enqueue_chapter_published, PgAuthoring};
use crate::services::authoring::{
    valid_command_key, AuthorError, ChapterAccess, PendingChapter, SaveChapter, SavedChapter,
};
use sha2::{Digest, Sha256};
use sqlx::Row;

impl PgAuthoring {
    /// Create a private, versioned chapter even though the parent book is public.
    /// Save and release are distinct, owner-only commands.
    pub async fn create_pending_free_chapter(
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
            || !(1..=50_000).contains(&input.body.chars().count())
            || !valid_command_key(&input.command_key)
        {
            return Err(AuthorError::Invalid);
        }

        // Domain-separate this key from the pre-publication save operation.
        let mut hasher = Sha256::new();
        for part in [
            "public-book-pending-chapter-v1".to_owned(),
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
        let digest: [u8; 32] = hasher.finalize().into();
        let body_hash: [u8; 32] = Sha256::digest(input.body.as_bytes()).into();
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
            Some("public") => (),
            Some(_) => return Err(AuthorError::Conflict),
        }

        let claimed: Option<i64> = sqlx::query_scalar(
            "INSERT INTO chapter_save_commands(user_id,command_key,payload_hash)
             VALUES ($1,$2,$3) ON CONFLICT(user_id,command_key) DO NOTHING
             RETURNING user_id",
        )
        .bind(actor)
        .bind(&input.command_key)
        .bind(digest.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if claimed.is_none() {
            let row = sqlx::query(
                "SELECT payload_hash,chapter_id,version_id,saved_revision
                 FROM chapter_save_commands
                 WHERE user_id=$1 AND command_key=$2 FOR UPDATE",
            )
            .bind(actor)
            .bind(&input.command_key)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
            let previous: Vec<u8> = row
                .try_get("payload_hash")
                .map_err(|_| AuthorError::Unavailable)?;
            if previous.as_slice() != digest.as_slice() {
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
             VALUES ($1,$2,$3,'draft','free',0,1) RETURNING id",
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
        .bind(body_hash.as_slice())
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        sqlx::query("INSERT INTO chapter_working_drafts(chapter_id,version_id) VALUES ($1,$2)")
            .bind(chapter_id)
            .bind(version_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
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

    /// Owner-only preview of the EXACT working version offered for release.
    pub async fn pending_free_chapter(
        &self,
        actor: i64,
        book_id: i64,
        chapter_id: i64,
    ) -> Result<Option<PendingChapter>, AuthorError> {
        if actor <= 0 || book_id <= 0 || chapter_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT c.chapter_no,c.title,c.edit_revision,d.version_id,v.body_markdown
             FROM chapters c JOIN books b ON b.id=c.book_id
             JOIN chapter_working_drafts d ON d.chapter_id=c.id
             JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=d.version_id
             WHERE c.id=$1 AND c.book_id=$2 AND b.owner_user_id=$3
               AND b.visibility='public' AND c.visibility='draft'
               AND c.access_level='free' AND c.price_amount=0
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND NOT c.admin_hold AND NOT c.is_deleted",
        )
        .bind(chapter_id)
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        row.map(|row| {
            Ok(PendingChapter {
                number: row
                    .try_get("chapter_no")
                    .map_err(|_| AuthorError::Unavailable)?,
                title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
                body: row
                    .try_get("body_markdown")
                    .map_err(|_| AuthorError::Unavailable)?,
                version_id: row
                    .try_get("version_id")
                    .map_err(|_| AuthorError::Unavailable)?,
                revision: row
                    .try_get("edit_revision")
                    .map_err(|_| AuthorError::Unavailable)?,
            })
        })
        .transpose()
    }

    /// Publish only the revision/version explicitly confirmed by the author.
    /// A second concurrent submission cannot emit a second first-release event.
    pub async fn release_pending_free_chapter(
        &self,
        actor: i64,
        book_id: i64,
        chapter_id: i64,
        expected_revision: i64,
        expected_version_id: i64,
    ) -> Result<(), AuthorError> {
        if actor <= 0 || book_id <= 0 || chapter_id <= 0 {
            return Err(AuthorError::Missing);
        }
        if expected_revision <= 0 || expected_version_id <= 0 {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
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
        match visibility.as_deref() {
            None => return Err(AuthorError::Missing),
            Some("public") => (),
            Some(_) => return Err(AuthorError::Conflict),
        }
        let row = sqlx::query(
            "SELECT visibility,edit_revision FROM chapters
             WHERE id=$1 AND book_id=$2 AND access_level='free' AND price_amount=0
               AND NOT admin_hold AND NOT is_deleted
               AND (reader_released_at IS NULL OR reader_released_at<=now())
             FOR UPDATE",
        )
        .bind(chapter_id)
        .bind(book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?
        .ok_or(AuthorError::Missing)?;
        let chapter_visibility: String = row
            .try_get("visibility")
            .map_err(|_| AuthorError::Unavailable)?;
        let revision: i64 = row
            .try_get("edit_revision")
            .map_err(|_| AuthorError::Unavailable)?;
        if revision != expected_revision {
            return Err(AuthorError::Conflict);
        }
        if chapter_visibility == "public" {
            // Exact replay after successful publication is safe and side-effect free.
            let current: Option<i64> = sqlx::query_scalar(
                "SELECT version_id FROM chapter_publications WHERE chapter_id=$1",
            )
            .bind(chapter_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
            return if current == Some(expected_version_id) {
                Ok(())
            } else {
                Err(AuthorError::Conflict)
            };
        }
        if chapter_visibility != "draft" {
            return Err(AuthorError::Conflict);
        }
        let version: Option<i64> =
            sqlx::query_scalar("SELECT version_id FROM chapter_working_drafts WHERE chapter_id=$1")
                .bind(chapter_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(|_| AuthorError::Unavailable)?;
        if version != Some(expected_version_id) {
            return Err(AuthorError::Conflict);
        }
        let inserted: Option<i64> = sqlx::query_scalar(
            "INSERT INTO chapter_publications(chapter_id,version_id)
             VALUES ($1,$2) ON CONFLICT(chapter_id) DO NOTHING RETURNING chapter_id",
        )
        .bind(chapter_id)
        .bind(expected_version_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if inserted.is_none() {
            return Err(AuthorError::Conflict);
        }
        sqlx::query("UPDATE chapters SET visibility='public' WHERE id=$1")
            .bind(chapter_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        sqlx::query("DELETE FROM chapter_working_drafts WHERE chapter_id=$1")
            .bind(chapter_id)
            .execute(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        sqlx::query(
            "INSERT INTO chapter_first_release_events(chapter_id,version_id)
             VALUES ($1,$2) ON CONFLICT(chapter_id) DO NOTHING",
        )
        .bind(chapter_id)
        .bind(expected_version_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        enqueue_chapter_published(
            &mut tx,
            book_id,
            chapter_id,
            expected_version_id,
            true,
        )
        .await?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(())
    }
}
