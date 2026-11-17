//! Two-phase immutable free editing. A failed upload leaves the old reader pointer.
use super::{write_error, PgFreeS3Publication};
use crate::services::authoring::{valid_command_key, AuthorError, ChapterAccess, SaveChapter};
use sha2::{Digest, Sha256};
use sqlx::Row;

pub struct PreparedFreeEdit {
    pub chapter_id: i64,
    pub version_id: i64,
    pub previous_version_id: i64,
    pub published: bool,
}

impl PgFreeS3Publication {
    pub async fn prepare_edit(
        &self,
        actor: i64,
        input: &SaveChapter,
    ) -> Result<PreparedFreeEdit, AuthorError> {
        let chapter = input.chapter_id.ok_or(AuthorError::Invalid)?;
        if actor <= 0
            || input.book_id <= 0
            || chapter <= 0
            || input.expected_revision < 1
            || input.number < 1
            || input.access != ChapterAccess::Free
            || input.price_amount != 0
            || !valid_command_key(&input.command_key)
            || !(1..=200).contains(&input.title.chars().count())
            || input.title.chars().any(char::is_control)
            || !(1..=50_000).contains(&input.body.chars().count())
            || input.body.contains(['\0', '\r'])
        {
            return Err(AuthorError::Invalid);
        }
        let mut hash = Sha256::new();
        for field in [
            "s349-free-edit-v1".to_owned(),
            input.book_id.to_string(),
            chapter.to_string(),
            input.expected_revision.to_string(),
            input.number.to_string(),
            input.title.clone(),
            input.body.clone(),
        ] {
            hash.update((field.len() as u64).to_be_bytes());
            hash.update(field.as_bytes());
        }
        let payload: [u8; 32] = hash.finalize().into();
        let content: [u8; 32] = Sha256::digest(input.body.as_bytes()).into();
        let mut tx = self.pool.begin().await.map_err(write_error)?;
        let book: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM books
            WHERE id=$1 AND owner_user_id=$2 AND visibility IN ('public','draft')
              AND NOT admin_hold AND NOT is_deleted FOR UPDATE",
        )
        .bind(input.book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
        book.ok_or(AuthorError::Missing)?;
        let row=sqlx::query("SELECT c.edit_revision,c.chapter_no,c.title,
            COALESCE(d.version_id,p.version_id) AS version_id,v.content_sha256
            FROM chapters c
            LEFT JOIN chapter_working_drafts d ON d.chapter_id=c.id
            LEFT JOIN chapter_publications p ON p.chapter_id=c.id
            JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=COALESCE(d.version_id,p.version_id)
            WHERE c.id=$1 AND c.book_id=$2 AND c.access_level='free' AND c.price_amount=0
              AND NOT c.admin_hold AND NOT c.is_deleted
              AND (
                EXISTS (
                  SELECT 1 FROM free_chapter_storage_objects f
                  WHERE f.chapter_id=v.chapter_id AND f.version_id=v.id
                    AND f.sha256=v.content_sha256
                )
                OR EXISTS (
                  SELECT 1 FROM free_edition_active_storage a
                  JOIN free_edition_storage_generations g
                    ON g.chapter_id=a.chapter_id AND g.version_id=a.version_id
                   AND g.generation=a.generation AND g.placement_kind='pack'
                  JOIN free_storage_packs pack ON pack.id=g.pack_id
                  WHERE a.chapter_id=v.chapter_id AND a.version_id=v.id
                    AND g.content_sha256=v.content_sha256
                )
              ) FOR UPDATE OF c")
            .bind(chapter).bind(input.book_id).fetch_optional(&mut *tx).await
            .map_err(write_error)?.ok_or(AuthorError::Missing)?;
        let current: i64 = row.try_get("version_id").map_err(write_error)?;
        let revision: i64 = row.try_get("edit_revision").map_err(write_error)?;
        let claimed: Option<i64> = sqlx::query_scalar(
            "INSERT INTO chapter_save_commands
            (user_id,command_key,payload_hash) VALUES($1,$2,$3)
            ON CONFLICT(user_id,command_key) DO NOTHING RETURNING user_id",
        )
        .bind(actor)
        .bind(&input.command_key)
        .bind(payload.as_slice())
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
        if claimed.is_none() {
            let old = sqlx::query(
                "SELECT s.payload_hash,s.chapter_id,s.version_id,
                EXISTS(SELECT 1 FROM free_chapter_storage_objects f
                    WHERE f.chapter_id=s.chapter_id AND f.version_id=s.version_id) AS finished
                FROM chapter_save_commands s WHERE user_id=$1 AND command_key=$2 FOR UPDATE",
            )
            .bind(actor)
            .bind(&input.command_key)
            .fetch_one(&mut *tx)
            .await
            .map_err(write_error)?;
            let old_hash: Vec<u8> = old.try_get("payload_hash").map_err(write_error)?;
            if old_hash != payload {
                return Err(AuthorError::Conflict);
            }
            let version_id: Option<i64> = old.try_get("version_id").map_err(write_error)?;
            let result = PreparedFreeEdit {
                chapter_id: chapter,
                version_id: version_id.ok_or(AuthorError::Conflict)?,
                previous_version_id: current,
                published: old.try_get("finished").map_err(write_error)?,
            };
            tx.commit().await.map_err(write_error)?;
            return Ok(result);
        }
        let number: i32 = row.try_get("chapter_no").map_err(write_error)?;
        if revision != input.expected_revision || number != input.number {
            return Err(AuthorError::Conflict);
        }
        let old_hash: Vec<u8> = row.try_get("content_sha256").map_err(write_error)?;
        let old_title: String = row.try_get("title").map_err(write_error)?;
        let unchanged = old_hash == content;
        let mut saved_revision = revision + 1;
        let version_id = if unchanged {
            if old_title == input.title {
                saved_revision = revision;
            } else {
                sqlx::query(
                    "UPDATE chapters SET title=$2,edit_revision=edit_revision+1 WHERE id=$1",
                )
                .bind(chapter)
                .bind(&input.title)
                .execute(&mut *tx)
                .await
                .map_err(write_error)?;
            }
            current
        } else {
            let next: i32 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(version_no),0)+1
                FROM chapter_versions WHERE chapter_id=$1",
            )
            .bind(chapter)
            .fetch_one(&mut *tx)
            .await
            .map_err(write_error)?;
            let version: i64 = sqlx::query_scalar(
                "INSERT INTO chapter_versions
                (chapter_id,version_no,content_sha256) VALUES($1,$2,$3) RETURNING id",
            )
            .bind(chapter)
            .bind(next)
            .bind(content.as_slice())
            .fetch_one(&mut *tx)
            .await
            .map_err(write_error)?;
            super::super::version_shape::insert(&mut tx, chapter, version, &input.body).await?;
            version
        };
        sqlx::query(
            "UPDATE chapter_save_commands SET chapter_id=$3,version_id=$4,saved_revision=$5
            WHERE user_id=$1 AND command_key=$2",
        )
        .bind(actor)
        .bind(&input.command_key)
        .bind(chapter)
        .bind(version_id)
        .bind(saved_revision)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        tx.commit().await.map_err(write_error)?;
        Ok(PreparedFreeEdit {
            chapter_id: chapter,
            version_id,
            previous_version_id: current,
            published: unchanged,
        })
    }

    pub async fn attach_edit(
        &self,
        actor: i64,
        input: &SaveChapter,
        prepared: &PreparedFreeEdit,
        metadata: (&str, [u8; 32], u64),
    ) -> Result<(), AuthorError> {
        let (bucket, digest, size) = metadata;
        if size != input.body.len() as u64 {
            return Err(AuthorError::Conflict);
        }
        let mut tx = self.pool.begin().await.map_err(write_error)?;
        let book: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM books WHERE id=$1 AND owner_user_id=$2
            AND NOT admin_hold AND NOT is_deleted AND visibility IN ('public','draft') FOR UPDATE",
        )
        .bind(input.book_id)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
        book.ok_or(AuthorError::Missing)?;
        let row = sqlx::query(
            "SELECT c.edit_revision,d.version_id AS draft,p.version_id AS publication
            FROM chapters c LEFT JOIN chapter_working_drafts d ON d.chapter_id=c.id
            LEFT JOIN chapter_publications p ON p.chapter_id=c.id
            WHERE c.id=$1 AND c.book_id=$2 AND c.access_level='free' AND c.price_amount=0
              AND NOT c.admin_hold AND NOT c.is_deleted FOR UPDATE OF c",
        )
        .bind(prepared.chapter_id)
        .bind(input.book_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?
        .ok_or(AuthorError::Missing)?;
        let finished: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM free_chapter_storage_objects
            WHERE chapter_id=$1 AND version_id=$2)",
        )
        .bind(prepared.chapter_id)
        .bind(prepared.version_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(write_error)?;
        if finished {
            tx.commit().await.map_err(write_error)?;
            return Ok(());
        }
        let revision: i64 = row.try_get("edit_revision").map_err(write_error)?;
        let draft: Option<i64> = row.try_get("draft").map_err(write_error)?;
        let publication: Option<i64> = row.try_get("publication").map_err(write_error)?;
        if revision != input.expected_revision
            || draft.or(publication) != Some(prepared.previous_version_id)
        {
            return Err(AuthorError::Conflict);
        }
        let hash: Vec<u8> = sqlx::query_scalar(
            "SELECT content_sha256 FROM chapter_versions
            WHERE chapter_id=$1 AND id=$2",
        )
        .bind(prepared.chapter_id)
        .bind(prepared.version_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(write_error)?;
        if hash != digest || digest != <[u8; 32]>::from(Sha256::digest(input.body.as_bytes())) {
            return Err(AuthorError::Conflict);
        }
        let key = format!(
            "private/free-chapters/{}/{}.md",
            prepared.chapter_id, prepared.version_id
        );
        sqlx::query(
            "INSERT INTO free_chapter_storage_objects
            (chapter_id,version_id,bucket,object_key,sha256,size_bytes) VALUES($1,$2,$3,$4,$5,$6)",
        )
        .bind(prepared.chapter_id)
        .bind(prepared.version_id)
        .bind(bucket)
        .bind(key)
        .bind(digest.as_slice())
        .bind(size as i64)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        let update = if draft.is_some() {
            "UPDATE chapter_working_drafts SET version_id=$2,updated_at=now() WHERE chapter_id=$1"
        } else {
            "UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1"
        };
        sqlx::query(update)
            .bind(prepared.chapter_id)
            .bind(prepared.version_id)
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
        sqlx::query("UPDATE chapters SET title=$2,edit_revision=edit_revision+1 WHERE id=$1")
            .bind(prepared.chapter_id)
            .bind(&input.title)
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
        tx.commit().await.map_err(write_error)?;
        Ok(())
    }
}
