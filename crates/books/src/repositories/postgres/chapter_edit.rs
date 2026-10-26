//! B-04: compare-and-swap revisions of an author's unpublished free chapter.
//! A published book is deliberately excluded until a separate release-of-edit
//! transaction exists. Editing must never silently change public reader text.
use super::PgAuthoring;
use crate::services::authoring::{
    valid_command_key, AuthorError, ChapterAccess, SaveChapter, SavedChapter,
};
use sha2::{Digest, Sha256};
use sqlx::Row;

impl PgAuthoring {
    /// Save a new immutable body version under an expected chapter revision.
    /// The book row and chapter row are locked in a stable order. Exact command
    /// replays return their previous result; altered commands conflict.
    pub async fn edit_unpublished_free_chapter(
        &self,
        actor: i64,
        input: &SaveChapter,
    ) -> Result<SavedChapter, AuthorError> {
        let chapter_id = input.chapter_id.ok_or(AuthorError::Invalid)?;
        if actor <= 0
            || input.book_id <= 0
            || chapter_id <= 0
            || input.number <= 0
            || input.expected_revision < 1
            || input.access != ChapterAccess::Free
            || input.price_amount != 0
            || !(1..=200).contains(&input.title.chars().count())
            || !(1..=50_000).contains(&input.body.chars().count())
            || !valid_command_key(&input.command_key)
        {
            return Err(AuthorError::Invalid);
        }

        let mut hasher = Sha256::new();
        for part in [
            "draft-chapter-edit-v1".to_string(),
            input.book_id.to_string(),
            chapter_id.to_string(),
            input.number.to_string(),
            input.expected_revision.to_string(),
            input.title.clone(),
            input.body.clone(),
            input.access.as_str().to_owned(),
            input.price_amount.to_string(),
        ] {
            hasher.update((part.len() as u64).to_be_bytes());
            hasher.update(part.as_bytes());
        }
        let payload_hash: [u8; 32] = hasher.finalize().into();
        let body_hash: [u8; 32] = Sha256::digest(input.body.as_bytes()).into();
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;

        // Match create_free_chapter's lock order: owner book before chapter.
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
            Some("draft") => (),
            // Updating a published pointer here would expose edits without a
            // distinct publishing decision. Reject until B-06 handles it.
            Some(_) => return Err(AuthorError::Conflict),
        }

        let claimed: Option<i64> = sqlx::query_scalar(
            "INSERT INTO chapter_save_commands (user_id,command_key,payload_hash)
             VALUES ($1,$2,$3) ON CONFLICT (user_id,command_key) DO NOTHING
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
                 FROM chapter_save_commands WHERE user_id=$1 AND command_key=$2
                 FOR UPDATE",
            )
            .bind(actor)
            .bind(&input.command_key)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
            let previous: Vec<u8> = row
                .try_get("payload_hash")
                .map_err(|_| AuthorError::Unavailable)?;
            if previous.as_slice() != payload_hash.as_slice() {
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

        let row = sqlx::query(
            "SELECT c.chapter_no,c.title,c.edit_revision,c.visibility,c.access_level,
                    c.price_amount,p.version_id,v.body_markdown
             FROM chapters c
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.id=p.version_id AND v.chapter_id=c.id
             WHERE c.id=$1 AND c.book_id=$2 AND NOT c.admin_hold AND NOT c.is_deleted
             FOR UPDATE OF c",
        )
        .bind(chapter_id)
        .bind(input.book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?
        .ok_or(AuthorError::Missing)?;
        let revision: i64 = row
            .try_get("edit_revision")
            .map_err(|_| AuthorError::Unavailable)?;
        let number: i32 = row
            .try_get("chapter_no")
            .map_err(|_| AuthorError::Unavailable)?;
        let level: String = row
            .try_get("access_level")
            .map_err(|_| AuthorError::Unavailable)?;
        let visibility: String = row
            .try_get("visibility")
            .map_err(|_| AuthorError::Unavailable)?;
        let price: i64 = row
            .try_get("price_amount")
            .map_err(|_| AuthorError::Unavailable)?;
        if revision != input.expected_revision
            || number != input.number
            || level != "free"
            || price != 0
            || visibility != "public"
        {
            return Err(AuthorError::Conflict);
        }

        let old_title: String = row.try_get("title").map_err(|_| AuthorError::Unavailable)?;
        let old_body: String = row
            .try_get("body_markdown")
            .map_err(|_| AuthorError::Unavailable)?;
        let old_version_id: i64 = row
            .try_get("version_id")
            .map_err(|_| AuthorError::Unavailable)?;
        let unchanged = old_title == input.title && old_body == input.body;
        let version_id = if old_body == input.body {
            old_version_id
        } else {
            let existing: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM chapter_versions
                 WHERE chapter_id=$1 AND content_sha256=$2",
            )
            .bind(chapter_id)
            .bind(body_hash.as_slice())
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
            if let Some(id) = existing {
                id
            } else {
                let next: i32 = sqlx::query_scalar(
                    "SELECT COALESCE(MAX(version_no),0)+1
                     FROM chapter_versions WHERE chapter_id=$1",
                )
                .bind(chapter_id)
                .fetch_one(&mut *tx)
                .await
                .map_err(|_| AuthorError::Unavailable)?;
                sqlx::query_scalar(
                    "INSERT INTO chapter_versions
                     (chapter_id,version_no,body_markdown,content_sha256)
                     VALUES ($1,$2,$3,$4) RETURNING id",
                )
                .bind(chapter_id)
                .bind(next)
                .bind(&input.body)
                .bind(body_hash.as_slice())
                .fetch_one(&mut *tx)
                .await
                .map_err(|_| AuthorError::Unavailable)?
            }
        };
        let saved_revision = if unchanged {
            revision
        } else {
            let next: Option<i64> = sqlx::query_scalar(
                "UPDATE chapters SET title=$2,edit_revision=edit_revision+1
                 WHERE id=$1 AND edit_revision=$3 RETURNING edit_revision",
            )
            .bind(chapter_id)
            .bind(&input.title)
            .bind(revision)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
            let new_revision = next.ok_or(AuthorError::Conflict)?;
            if version_id != old_version_id {
                sqlx::query(
                    "UPDATE chapter_publications SET version_id=$2
                     WHERE chapter_id=$1",
                )
                .bind(chapter_id)
                .bind(version_id)
                .execute(&mut *tx)
                .await
                .map_err(|_| AuthorError::Unavailable)?;
            }
            new_revision
        };
        sqlx::query(
            "UPDATE chapter_save_commands
             SET chapter_id=$3,version_id=$4,saved_revision=$5
             WHERE user_id=$1 AND command_key=$2",
        )
        .bind(actor)
        .bind(&input.command_key)
        .bind(chapter_id)
        .bind(version_id)
        .bind(saved_revision)
        .execute(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(SavedChapter {
            chapter_id,
            version_id,
            revision: saved_revision,
        })
    }

    /// Read only the current working version of the owner's unpublished chapter.
    /// A public book, another author and a deleted/held chapter all look absent.
    pub async fn owned_unpublished_free_chapter(
        &self,
        actor: i64,
        book_id: i64,
        chapter_id: i64,
    ) -> Result<Option<crate::services::authoring::OwnedChapter>, AuthorError> {
        use crate::services::authoring::OwnedChapter;
        if actor <= 0 || book_id <= 0 || chapter_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT c.chapter_no,c.title,c.edit_revision,v.body_markdown
             FROM chapters c
             JOIN books b ON b.id=c.book_id
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=p.version_id
             WHERE c.id=$1 AND c.book_id=$2 AND b.owner_user_id=$3
               AND b.visibility='draft' AND NOT b.admin_hold AND NOT b.is_deleted
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND c.access_level='free' AND c.price_amount=0",
        )
        .bind(chapter_id)
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        row.map(|row| {
            Ok(OwnedChapter {
                number: row
                    .try_get("chapter_no")
                    .map_err(|_| AuthorError::Unavailable)?,
                title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
                body: row
                    .try_get("body_markdown")
                    .map_err(|_| AuthorError::Unavailable)?,
                access: ChapterAccess::Free,
                price_amount: 0,
                revision: row
                    .try_get("edit_revision")
                    .map_err(|_| AuthorError::Unavailable)?,
            })
        })
        .transpose()
    }

}
