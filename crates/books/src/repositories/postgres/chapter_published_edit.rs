//! BKR-96: one Save immediately activates an immutable version of an already
//! published FREE chapter. A purchased/paid chapter is never editable here.
use super::PgAuthoring;
use crate::services::authoring::{
    valid_command_key, AuthorError, ChapterAccess, OwnedChapter, SaveChapter, SavedChapter,
};
use sha2::{Digest, Sha256};
use sqlx::Row;

fn write_error(error: sqlx::Error) -> AuthorError {
    if let sqlx::Error::Database(ref db) = error {
        if matches!(db.code().as_deref(), Some("23503" | "23505" | "23514")) {
            return AuthorError::Conflict;
        }
    }
    AuthorError::Unavailable
}

impl PgAuthoring {
    /// Load only an owner-controlled, currently published free chapter. The
    /// public reader always resolves the current version via B's publication.
    pub async fn owned_published_free_chapter(
        &self,
        actor: i64,
        book_id: i64,
        chapter_id: i64,
    ) -> Result<Option<OwnedChapter>, AuthorError> {
        if actor <= 0 || book_id <= 0 || chapter_id <= 0 {
            return Ok(None);
        }
        let row = sqlx::query(
            "SELECT c.chapter_no,c.title,c.edit_revision,v.body_markdown
             FROM chapters c
             JOIN books b ON b.id=c.book_id
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.id=p.version_id AND v.chapter_id=c.id
             WHERE c.id=$1 AND c.book_id=$2 AND b.owner_user_id=$3
               AND b.visibility='public' AND c.visibility='public'
               AND c.access_level='free' AND c.price_amount=0
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND NOT EXISTS (
                   SELECT 1 FROM chapter_working_drafts d WHERE d.chapter_id=c.id
               )",
        )
        .bind(chapter_id)
        .bind(book_id)
        .bind(actor)
        .fetch_optional(&self.pool)
        .await
        .map_err(write_error)?;
        row.map(|row| {
            Ok(OwnedChapter {
                number: row.try_get("chapter_no").map_err(|_| AuthorError::Unavailable)?,
                title: row.try_get("title").map_err(|_| AuthorError::Unavailable)?,
                body: row.try_get("body_markdown").map_err(|_| AuthorError::Unavailable)?,
                access: ChapterAccess::Free,
                price_amount: 0,
                revision: row.try_get("edit_revision").map_err(|_| AuthorError::Unavailable)?,
            })
        })
        .transpose()
    }

    /// Exact command replay is idempotent. Different commands racing from two
    /// tabs are serialized by the owner book/chapter locks and the expected
    /// edit_revision; a stale tab receives 409 without changing any metadata.
    /// A new body yields one immutable B edition and changes the reader pointer
    /// in the SAME PG transaction. Title-only and no-op saves do not invent
    /// chapter versions; the first-release event is never rewritten.
    pub async fn edit_published_free_chapter(
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
            || !valid_command_key(&input.command_key)
            || !(1..=200).contains(&input.title.chars().count())
            || input.title.chars().any(char::is_control)
            || !(1..=50_000).contains(&input.body.chars().count())
            || input.body.contains('\0')
            || input.body.contains('\r')
        {
            return Err(AuthorError::Invalid);
        }
        let mut hasher = Sha256::new();
        for part in [
            "published-free-chapter-edit-v1".to_owned(),
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
        let command_digest: [u8; 32] = hasher.finalize().into();
        let body_digest: [u8; 32] = Sha256::digest(input.body.as_bytes()).into();
        let mut tx = self.pool.begin().await.map_err(write_error)?;
        let visibility: Option<String> = sqlx::query_scalar(
            "SELECT visibility FROM books
             WHERE id=$1 AND owner_user_id=$2 AND NOT admin_hold AND NOT is_deleted
             FOR UPDATE",
        )
        .bind(input.book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
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
        .bind(command_digest.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
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
            .map_err(write_error)?;
            let previous: Vec<u8> = row.try_get("payload_hash").map_err(|_| AuthorError::Unavailable)?;
            if previous.as_slice() != command_digest.as_slice() {
                return Err(AuthorError::Conflict);
            }
            let previous_chapter: Option<i64> =
                row.try_get("chapter_id").map_err(|_| AuthorError::Unavailable)?;
            if previous_chapter != Some(chapter_id) {
                return Err(AuthorError::Conflict);
            }
            return Ok(SavedChapter {
                chapter_id,
                version_id: row
                    .try_get::<Option<i64>, _>("version_id")
                    .map_err(|_| AuthorError::Unavailable)?
                    .ok_or(AuthorError::Conflict)?,
                revision: row
                    .try_get::<Option<i64>, _>("saved_revision")
                    .map_err(|_| AuthorError::Unavailable)?
                    .ok_or(AuthorError::Conflict)?,
            });
        }
        let row = sqlx::query(
            "SELECT c.chapter_no,c.title,c.edit_revision,p.version_id,
                    v.body_markdown
             FROM chapters c
             JOIN chapter_publications p ON p.chapter_id=c.id
             JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=p.version_id
             WHERE c.id=$1 AND c.book_id=$2
               AND c.visibility='public' AND c.access_level='free'
               AND c.price_amount=0 AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at<=now())
               AND NOT EXISTS(
                   SELECT 1 FROM chapter_working_drafts d WHERE d.chapter_id=c.id
               )
             FOR UPDATE OF c,p",
        )
        .bind(chapter_id)
        .bind(input.book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?
        .ok_or(AuthorError::Missing)?;
        let revision: i64 = row.try_get("edit_revision").map_err(|_| AuthorError::Unavailable)?;
        let number: i32 = row.try_get("chapter_no").map_err(|_| AuthorError::Unavailable)?;
        if revision != input.expected_revision || number != input.number {
            return Err(AuthorError::Conflict);
        }
        let old_title: String = row.try_get("title").map_err(|_| AuthorError::Unavailable)?;
        let old_body: String = row.try_get("body_markdown").map_err(|_| AuthorError::Unavailable)?;
        let old_version: i64 = row.try_get("version_id").map_err(|_| AuthorError::Unavailable)?;
        let same_body = old_body == input.body;
        let unchanged = old_title == input.title && same_body;
        let version_id = if same_body {
            old_version
        } else {
            let existing: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM chapter_versions
                 WHERE chapter_id=$1 AND content_sha256=$2",
            )
            .bind(chapter_id)
            .bind(body_digest.as_slice())
            .fetch_optional(&mut *tx)
            .await
            .map_err(write_error)?;
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
                .map_err(write_error)?;
                sqlx::query_scalar(
                    "INSERT INTO chapter_versions
                     (chapter_id,version_no,body_markdown,content_sha256)
                     VALUES ($1,$2,$3,$4) RETURNING id",
                )
                .bind(chapter_id)
                .bind(next)
                .bind(&input.body)
                .bind(body_digest.as_slice())
                .fetch_one(&mut *tx)
                .await
                .map_err(write_error)?
            }
        };
        let saved_revision = if unchanged {
            revision
        } else {
            let next: Option<i64> = sqlx::query_scalar(
                "UPDATE chapters SET title=$2,edit_revision=edit_revision+1
                 WHERE id=$1 AND edit_revision=$3 AND visibility='public'
                   AND access_level='free' AND price_amount=0
                 RETURNING edit_revision",
            )
            .bind(chapter_id)
            .bind(&input.title)
            .bind(revision)
            .fetch_optional(&mut *tx)
            .await
            .map_err(write_error)?;
            let new_revision = next.ok_or(AuthorError::Conflict)?;
            if version_id != old_version {
                let changed = sqlx::query(
                    "UPDATE chapter_publications
                     SET version_id=$3,published_at=now()
                     WHERE chapter_id=$1 AND version_id=$2",
                )
                .bind(chapter_id)
                .bind(old_version)
                .bind(version_id)
                .execute(&mut *tx)
                .await
                .map_err(write_error)?;
                if changed.rows_affected() != 1 {
                    return Err(AuthorError::Conflict);
                }
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
        .map_err(write_error)?;
        tx.commit().await.map_err(write_error)?;
        Ok(SavedChapter {
            chapter_id,
            version_id,
            revision: saved_revision,
        })
    }
}
