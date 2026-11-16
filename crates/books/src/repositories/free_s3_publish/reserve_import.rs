//! Reserve sequence identities and bounded source metadata, never live chapters.
use super::{write_error, PgFreeS3Publication, VerifiedFreeImportEdition};
use crate::services::{
    authoring::AuthorError,
    chapter_file_import::{fingerprint, ImportedChapter},
};
use sha2::{Digest, Sha256};
use sqlx::Row;

pub struct PreparedFreeImportEdition {
    pub chapter_id: i64,
    pub version_id: i64,
    pub verified: bool,
}

impl PgFreeS3Publication {
    pub async fn prepare_import(
        &self,
        actor: i64,
        book: i64,
        key: &str,
        chapters: &[ImportedChapter],
    ) -> Result<Vec<PreparedFreeImportEdition>, AuthorError> {
        super::super::postgres::chapter_batch_import::validate_batch(actor, book, key, chapters)?;
        let digest = fingerprint(book, chapters);
        let mut tx = self.pool.begin().await.map_err(write_error)?;
        let visibility: Option<String> = sqlx::query_scalar(
            "SELECT visibility FROM books
            WHERE id=$1 AND owner_user_id=$2 AND NOT admin_hold AND NOT is_deleted FOR UPDATE",
        )
        .bind(book)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
        let visibility = visibility.ok_or(AuthorError::Missing)?;
        let old=sqlx::query("SELECT r.chapter_id,r.version_id,r.book_id,r.batch_sha256,
            EXISTS(SELECT 1 FROM free_chapter_import_uploads f
                WHERE f.version_id=r.version_id AND f.content_sha256=r.content_sha256
                  AND f.size_bytes=r.size_bytes) AS finished
            FROM free_chapter_import_reservations r WHERE user_id=$1 AND command_key=$2 ORDER BY chapter_index")
            .bind(actor).bind(key).fetch_all(&mut *tx).await.map_err(write_error)?;
        if !old.is_empty() {
            if old.len() != chapters.len() {
                return Err(AuthorError::Conflict);
            }
            let mut result = Vec::with_capacity(old.len());
            for row in old {
                let hash: Vec<u8> = row.try_get("batch_sha256").map_err(write_error)?;
                let old_book: i64 = row.try_get("book_id").map_err(write_error)?;
                if old_book != book || hash != digest {
                    return Err(AuthorError::Conflict);
                }
                result.push(PreparedFreeImportEdition {
                    chapter_id: row.try_get("chapter_id").map_err(write_error)?,
                    version_id: row.try_get("version_id").map_err(write_error)?,
                    verified: row.try_get("finished").map_err(write_error)?,
                });
            }
            tx.commit().await.map_err(write_error)?;
            return Ok(result);
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
        .fetch_one(&mut *tx)
        .await
        .map_err(write_error)?;
        if used {
            return Err(AuthorError::Conflict);
        }
        let mut result = Vec::with_capacity(chapters.len());
        for (index, source) in chapters.iter().enumerate() {
            let shape = super::super::version_shape::shape(&source.body)?;
            let content: [u8; 32] = Sha256::digest(source.body.as_bytes()).into();
            let row=sqlx::query("INSERT INTO free_chapter_import_reservations
                (user_id,command_key,book_id,chapter_index,chapter_id,version_id,chapter_no,title,
                 batch_sha256,content_sha256,size_bytes,block_utf16_lengths,last_readable_block_index,last_readable_char_offset)
                VALUES($1,$2,$3,$4,nextval(pg_get_serial_sequence('chapters','id')),
                    nextval(pg_get_serial_sequence('chapter_versions','id')),$5,$6,$7,$8,$9,$10,$11,$12)
                RETURNING chapter_id,version_id")
                .bind(actor).bind(key).bind(book).bind(index as i32).bind(source.number).bind(&source.title)
                .bind(digest.as_slice()).bind(content.as_slice()).bind(source.body.len() as i64)
                .bind(&shape.block_utf16_lengths).bind(shape.last_readable_block_index).bind(shape.last_readable_char_offset)
                .fetch_one(&mut *tx).await.map_err(write_error)?;
            result.push(PreparedFreeImportEdition {
                chapter_id: row.try_get("chapter_id").map_err(write_error)?,
                version_id: row.try_get("version_id").map_err(write_error)?,
                verified: false,
            });
        }
        tx.commit().await.map_err(write_error)?;
        Ok(result)
    }

    pub async fn mark_import_verified(
        &self,
        actor: i64,
        key: &str,
        bucket: &str,
        edition: &VerifiedFreeImportEdition,
    ) -> Result<(), AuthorError> {
        sqlx::query("INSERT INTO free_chapter_import_uploads(version_id,bucket,content_sha256,size_bytes)
            SELECT r.version_id,$3,r.content_sha256,r.size_bytes FROM free_chapter_import_reservations r
            WHERE r.user_id=$1 AND r.command_key=$2 AND r.version_id=$4 AND r.chapter_id=$5
              AND r.content_sha256=$6 AND r.size_bytes=$7 ON CONFLICT(version_id) DO NOTHING")
            .bind(actor).bind(key).bind(bucket).bind(edition.version_id).bind(edition.chapter_id)
            .bind(edition.sha256.as_slice()).bind(edition.size_bytes as i64)
            .execute(&self.pool).await.map_err(write_error)?;
        let exact: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM free_chapter_import_uploads
            WHERE version_id=$1 AND bucket=$2 AND content_sha256=$3 AND size_bytes=$4)",
        )
        .bind(edition.version_id)
        .bind(bucket)
        .bind(edition.sha256.as_slice())
        .bind(edition.size_bytes as i64)
        .fetch_one(&self.pool)
        .await
        .map_err(write_error)?;
        if exact {
            Ok(())
        } else {
            Err(AuthorError::Conflict)
        }
    }
}
