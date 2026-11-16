//! Uploads precede one atomic B import transaction. No connection is held during S3.
use super::{write_error, PgFreeS3Publication};
use crate::services::{
    authoring::{valid_command_key, AuthorError},
    chapter_file_import::{fingerprint, ImportedChapter, MAX_IMPORT_CHAPTERS},
};
use sqlx::{PgConnection, Row};

pub struct VerifiedFreeImportEdition {
    pub chapter_id: i64,
    pub version_id: i64,
    pub sha256: [u8; 32],
    pub size_bytes: u64,
}

impl PgFreeS3Publication {
    /// Job fence, all B pointers and G succeeded state commit together.
    pub async fn finish_import_job(
        &self,
        job: (i64, i64),
        chapters: &[ImportedChapter],
        bucket: &str,
        editions: &[VerifiedFreeImportEdition],
    ) -> Result<usize, AuthorError> {
        let (job_id, epoch) = job;
        if job_id <= 0 || epoch <= 0 {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(write_error)?;
        let row=sqlx::query("SELECT status,lease_epoch,actor_user_id,book_id,command_key,
            approved_fingerprint,expected_chapter_count,COALESCE(locked_until>now(),false) AS lease_valid
            FROM chapter_import_jobs WHERE id=$1 FOR UPDATE")
            .bind(job_id).fetch_optional(&mut *tx).await.map_err(write_error)?.ok_or(AuthorError::Conflict)?;
        let status: String = row.try_get("status").map_err(write_error)?;
        let current: i64 = row.try_get("lease_epoch").map_err(write_error)?;
        let actor: i64 = row.try_get("actor_user_id").map_err(write_error)?;
        let book: i64 = row.try_get("book_id").map_err(write_error)?;
        let key: String = row.try_get("command_key").map_err(write_error)?;
        let hash: Vec<u8> = row.try_get("approved_fingerprint").map_err(write_error)?;
        let count: i32 = row.try_get("expected_chapter_count").map_err(write_error)?;
        let valid: bool = row.try_get("lease_valid").map_err(write_error)?;
        if status != "running"
            || current != epoch
            || !valid
            || count as usize != chapters.len()
            || hash != fingerprint(book, chapters)
        {
            return Err(AuthorError::Conflict);
        }
        let saved = finish_transaction(
            &mut tx,
            ImportBatch {
                actor,
                book,
                key: &key,
                chapters,
                bucket,
                editions,
            },
        )
        .await?;
        let changed = sqlx::query(
            "UPDATE chapter_import_jobs SET status='succeeded',locked_until=NULL,
            saved_count=$3,last_error_code=NULL,updated_at=now(),finished_at=now()
            WHERE id=$1 AND lease_epoch=$2 AND status='running' AND locked_until>now()",
        )
        .bind(job_id)
        .bind(epoch)
        .bind(saved as i32)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        if changed.rows_affected() != 1 {
            return Err(AuthorError::Conflict);
        }
        tx.commit().await.map_err(write_error)?;
        Ok(saved)
    }

    pub async fn finish_import(
        &self,
        actor: i64,
        book: i64,
        key: &str,
        chapters: &[ImportedChapter],
        bucket: &str,
        editions: &[VerifiedFreeImportEdition],
    ) -> Result<usize, AuthorError> {
        let mut tx = self.pool.begin().await.map_err(write_error)?;
        let saved = finish_transaction(
            &mut tx,
            ImportBatch {
                actor,
                book,
                key,
                chapters,
                bucket,
                editions,
            },
        )
        .await?;
        tx.commit().await.map_err(write_error)?;
        Ok(saved)
    }
}

struct ImportBatch<'a> {
    actor: i64,
    book: i64,
    key: &'a str,
    chapters: &'a [ImportedChapter],
    bucket: &'a str,
    editions: &'a [VerifiedFreeImportEdition],
}

async fn finish_transaction(
    connection: &mut PgConnection,
    batch: ImportBatch<'_>,
) -> Result<usize, AuthorError> {
    let ImportBatch {
        actor,
        book,
        key,
        chapters,
        bucket,
        editions,
    } = batch;
    if actor <= 0
        || book <= 0
        || !valid_command_key(key)
        || chapters.is_empty()
        || chapters.len() > MAX_IMPORT_CHAPTERS
        || chapters.len() != editions.len()
    {
        return Err(AuthorError::Invalid);
    }
    let digest = fingerprint(book, chapters);
    let visibility: Option<String> = sqlx::query_scalar(
        "SELECT visibility FROM books
            WHERE id=$1 AND owner_user_id=$2 AND NOT admin_hold AND NOT is_deleted FOR UPDATE",
    )
    .bind(book)
    .bind(actor)
    .fetch_optional(&mut *connection)
    .await
    .map_err(write_error)?;
    let visibility = visibility.ok_or(AuthorError::Missing)?;
    if let Some(old) = sqlx::query(
        "SELECT book_id,content_sha256,chapter_count
            FROM chapter_import_batches WHERE user_id=$1 AND command_key=$2 FOR UPDATE",
    )
    .bind(actor)
    .bind(key)
    .fetch_optional(&mut *connection)
    .await
    .map_err(write_error)?
    {
        let old_book: i64 = old.try_get("book_id").map_err(write_error)?;
        let old_hash: Vec<u8> = old.try_get("content_sha256").map_err(write_error)?;
        let count: i32 = old.try_get("chapter_count").map_err(write_error)?;
        if old_book != book || old_hash != digest || count as usize != chapters.len() {
            return Err(AuthorError::Conflict);
        }
        return Ok(chapters.len());
    }
    if visibility != "draft" {
        return Err(AuthorError::Conflict);
    }
    let numbers: Vec<i32> = chapters.iter().map(|c| c.number).collect();
    let used: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM chapters WHERE book_id=$1 AND chapter_no=ANY($2::int[]))",
    )
    .bind(book)
    .bind(&numbers)
    .fetch_one(&mut *connection)
    .await
    .map_err(write_error)?;
    if used {
        return Err(AuthorError::Conflict);
    }
    for (source, edition) in chapters.iter().zip(editions) {
        let owned: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM free_chapter_import_reservations r
                JOIN free_chapter_import_uploads u ON u.version_id=r.version_id
                WHERE r.user_id=$1 AND r.book_id=$2 AND r.command_key=$3
                  AND r.chapter_id=$4 AND r.version_id=$5 AND r.chapter_no=$6 AND r.title=$7
                  AND r.content_sha256=$8 AND r.batch_sha256=$9
                  AND r.size_bytes=$10 AND u.bucket=$11
                  AND u.content_sha256=r.content_sha256 AND u.size_bytes=r.size_bytes)",
        )
        .bind(actor)
        .bind(book)
        .bind(key)
        .bind(edition.chapter_id)
        .bind(edition.version_id)
        .bind(source.number)
        .bind(&source.title)
        .bind(edition.sha256.as_slice())
        .bind(digest.as_slice())
        .bind(edition.size_bytes as i64)
        .bind(bucket)
        .fetch_one(&mut *connection)
        .await
        .map_err(write_error)?;
        if !owned {
            return Err(AuthorError::Conflict);
        }
        sqlx::query(
            "INSERT INTO chapters
                (id,book_id,chapter_no,title,visibility,access_level,price_amount,edit_revision)
                VALUES($1,$2,$3,$4,'public','free',0,1)",
        )
        .bind(edition.chapter_id)
        .bind(book)
        .bind(source.number)
        .bind(&source.title)
        .execute(&mut *connection)
        .await
        .map_err(write_error)?;
        sqlx::query(
            "INSERT INTO chapter_versions(id,chapter_id,version_no,content_sha256)
                VALUES($1,$2,1,$3)",
        )
        .bind(edition.version_id)
        .bind(edition.chapter_id)
        .bind(edition.sha256.as_slice())
        .execute(&mut *connection)
        .await
        .map_err(write_error)?;
        sqlx::query("INSERT INTO chapter_version_content_shapes
                (chapter_id,version_id,content_sha256,block_utf16_lengths,last_readable_block_index,last_readable_char_offset)
                SELECT chapter_id,version_id,content_sha256,block_utf16_lengths,last_readable_block_index,last_readable_char_offset
                FROM free_chapter_import_reservations WHERE version_id=$1")
                .bind(edition.version_id).execute(&mut *connection).await.map_err(write_error)?;
        let object_key = format!(
            "private/free-chapters/{}/{}.md",
            edition.chapter_id, edition.version_id
        );
        sqlx::query("INSERT INTO free_chapter_storage_objects
                (chapter_id,version_id,bucket,object_key,sha256,size_bytes) VALUES($1,$2,$3,$4,$5,$6)")
                .bind(edition.chapter_id).bind(edition.version_id).bind(bucket).bind(object_key)
                .bind(edition.sha256.as_slice()).bind(edition.size_bytes as i64)
                .execute(&mut *connection).await.map_err(write_error)?;
        sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
            .bind(edition.chapter_id)
            .bind(edition.version_id)
            .execute(&mut *connection)
            .await
            .map_err(write_error)?;
        sqlx::query("UPDATE chapters SET visibility='public' WHERE id=$1")
            .bind(edition.chapter_id)
            .execute(&mut *connection)
            .await
            .map_err(write_error)?;
    }
    sqlx::query(
        "INSERT INTO chapter_import_batches
            (user_id,command_key,book_id,content_sha256,chapter_count) VALUES($1,$2,$3,$4,$5)",
    )
    .bind(actor)
    .bind(key)
    .bind(book)
    .bind(digest.as_slice())
    .bind(chapters.len() as i32)
    .execute(&mut *connection)
    .await
    .map_err(write_error)?;
    Ok(chapters.len())
}
