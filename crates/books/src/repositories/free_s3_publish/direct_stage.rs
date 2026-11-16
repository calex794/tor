//! Original DROP-04a: attach an independently verified, direct PACK stage
//! to a S023 import job. There is intentionally NO SINGLE PUT and NO chapter
//! publication in this method; full finalization requires a separately
//! accepted DROP-04b transactional publisher and authorized PACK reader.
use super::{write_error, PgFreeS3Publication};
use crate::services::{
    authoring::AuthorError,
    chapter_file_import::{fingerprint, ImportedChapter, MAX_IMPORT_CHAPTERS},
};
use sha2::{Digest, Sha256};
use sqlx::Row;
use tor_contracts::drop_pack::PackPlacementReceipt;

pub struct DirectPackStageRequest<'a> {
    pub job_id: i64,
    pub epoch: i64,
    pub actor: i64,
    pub book: i64,
    pub command_key: &'a str,
    pub chapters: &'a [ImportedChapter],
    pub expected_bucket: &'a str,
    /// G already performed an independent full S3 readback and verified every
    /// frame against these exact parsed source bytes. Never user-supplied.
    pub receipt: &'a PackPlacementReceipt,
}

impl PgFreeS3Publication {
    pub async fn stage_verified_direct_pack(
        &self,
        req: DirectPackStageRequest<'_>,
    ) -> Result<(), AuthorError> {
        let DirectPackStageRequest {
            job_id, epoch, actor, book, command_key, chapters, expected_bucket, receipt,
        } = req;
        let target = &receipt.target;
        if job_id <= 0 || epoch <= 0 || actor <= 0 || book <= 0
            || chapters.is_empty() || chapters.len() > MAX_IMPORT_CHAPTERS
            || chapters.len() != receipt.frames.len()
            || expected_bucket.is_empty()
            || expected_bucket.starts_with("public")
            || target.bucket != expected_bucket
            || !target.pack_key.starts_with("private/free-packs/import/")
            || target.pack_key.contains("//")
            || !target.pack_key.ends_with(".zst")
            || target.manifest_key != target.pack_key.replace(".zst", ".manifest.json")
            || target.pack_sha256 == [0; 32]
            || target.manifest_sha256 == [0; 32]
            || !(26..=16 * 1024 * 1024).contains(&target.pack_bytes)
            || !(1..=1024 * 1024).contains(&target.manifest_bytes)
            || target.seek_table_start == 0
            || target.seek_table_start >= target.pack_bytes
        {
            return Err(AuthorError::Invalid);
        }
        let mut seen = std::collections::HashSet::new();
        let mut offset = 0_u64;
        for frame in &receipt.frames {
            if frame.chapter_id <= 0 || frame.version_id <= 0
                || frame.content_sha256 == [0; 32]
                || !(1..=200_000).contains(&frame.size_bytes)
                || !(1..=204_096).contains(&frame.frame_bytes)
                || frame.frame_offset != offset
                || !seen.insert((frame.chapter_id, frame.version_id))
            {
                return Err(AuthorError::Invalid);
            }
            offset = offset.checked_add(frame.frame_bytes)
                .ok_or(AuthorError::Invalid)?;
        }
        if offset != target.seek_table_start {
            return Err(AuthorError::Invalid);
        }
        let approved = fingerprint(book, chapters);
        let receipt_value = serde_json::to_value(receipt).map_err(|_| AuthorError::Invalid)?;
        let receipt_json = serde_json::to_string(&receipt_value).map_err(|_| AuthorError::Invalid)?;
        let mut tx = self.pool.begin().await.map_err(write_error)?;
        let job = sqlx::query(
            "SELECT actor_user_id,book_id,command_key,status,lease_epoch,
                    approved_fingerprint,expected_chapter_count,
                    locked_until>clock_timestamp() AS lease_valid
             FROM chapter_import_jobs WHERE id=$1 FOR UPDATE"
        )
        .bind(job_id).fetch_optional(&mut *tx).await
        .map_err(write_error)?.ok_or(AuthorError::Conflict)?;
        if job.try_get::<i64,_>("actor_user_id").map_err(write_error)? != actor
            || job.try_get::<i64,_>("book_id").map_err(write_error)? != book
            || job.try_get::<String,_>("command_key").map_err(write_error)? != command_key
            || job.try_get::<String,_>("status").map_err(write_error)? != "running"
            || job.try_get::<i64,_>("lease_epoch").map_err(write_error)? != epoch
            || !job.try_get::<bool,_>("lease_valid").map_err(write_error)?
            || job.try_get::<Vec<u8>,_>("approved_fingerprint").map_err(write_error)?
                != approved
            || job.try_get::<i32,_>("expected_chapter_count").map_err(write_error)?
                != chapters.len() as i32
        {
            return Err(AuthorError::Conflict);
        }
        let owner: Option<i64> = sqlx::query_scalar(
            "SELECT b.id FROM books b JOIN users u ON u.id=b.owner_user_id
             WHERE b.id=$1 AND b.owner_user_id=$2 AND b.visibility='draft'
               AND u.account_state='active' AND NOT b.is_deleted AND NOT b.admin_hold
             FOR NO KEY UPDATE OF b,u"
        ).bind(book).bind(actor).fetch_optional(&mut *tx).await.map_err(write_error)?;
        if owner.is_none() {
            return Err(AuthorError::Conflict);
        }
        let rows = sqlx::query(
            "SELECT chapter_index,chapter_id,version_id,chapter_no,title,
                    batch_sha256,content_sha256,size_bytes
             FROM free_chapter_import_reservations
             WHERE user_id=$1 AND book_id=$2 AND command_key=$3
             ORDER BY chapter_index FOR NO KEY UPDATE"
        ).bind(actor).bind(book).bind(command_key)
          .fetch_all(&mut *tx).await.map_err(write_error)?;
        if rows.len() != chapters.len() {
            return Err(AuthorError::Conflict);
        }
        for (i, ((row, chapter), frame)) in rows.iter()
            .zip(chapters).zip(&receipt.frames).enumerate()
        {
            let content: [u8; 32] = Sha256::digest(chapter.body.as_bytes()).into();
            let exact = row.try_get::<i32,_>("chapter_index").map_err(write_error)? == i as i32
                && row.try_get::<i64,_>("chapter_id").map_err(write_error)? == frame.chapter_id
                && row.try_get::<i64,_>("version_id").map_err(write_error)? == frame.version_id
                && row.try_get::<i32,_>("chapter_no").map_err(write_error)? == chapter.number
                && row.try_get::<String,_>("title").map_err(write_error)? == chapter.title
                && row.try_get::<Vec<u8>,_>("batch_sha256").map_err(write_error)? == approved
                && row.try_get::<Vec<u8>,_>("content_sha256").map_err(write_error)? == content
                && content == frame.content_sha256
                && row.try_get::<i64,_>("size_bytes").map_err(write_error)?
                    == chapter.body.len() as i64
                && frame.size_bytes == chapter.body.len() as u64;
            if !exact {
                return Err(AuthorError::Conflict);
            }
        }
        sqlx::query(
            "INSERT INTO free_pack_direct_import_stages
             (job_id,actor_user_id,book_id,approved_fingerprint,chapter_count,
              bucket,object_key,pack_sha256,manifest_sha256,receipt)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10::jsonb)
             ON CONFLICT(job_id) DO NOTHING"
        )
        .bind(job_id).bind(actor).bind(book).bind(approved.as_slice())
        .bind(chapters.len() as i32)
        .bind(expected_bucket).bind(&target.pack_key)
        .bind(target.pack_sha256.as_slice()).bind(target.manifest_sha256.as_slice())
        .bind(&receipt_json).execute(&mut *tx).await.map_err(write_error)?;
        let row = sqlx::query(
            "SELECT actor_user_id,book_id,approved_fingerprint,chapter_count,bucket,
                    object_key,pack_sha256,manifest_sha256,receipt::text AS receipt_json
             FROM free_pack_direct_import_stages WHERE job_id=$1 FOR SHARE"
        ).bind(job_id).fetch_one(&mut *tx).await.map_err(write_error)?;
        let identical = row.try_get::<i64,_>("actor_user_id").map_err(write_error)? == actor
            && row.try_get::<i64,_>("book_id").map_err(write_error)? == book
            && row.try_get::<Vec<u8>,_>("approved_fingerprint").map_err(write_error)? == approved
            && row.try_get::<i32,_>("chapter_count").map_err(write_error)? == chapters.len() as i32
            && row.try_get::<String,_>("bucket").map_err(write_error)? == expected_bucket
            && row.try_get::<String,_>("object_key").map_err(write_error)? == target.pack_key
            && row.try_get::<Vec<u8>,_>("pack_sha256").map_err(write_error)?
                == target.pack_sha256
            && row.try_get::<Vec<u8>,_>("manifest_sha256").map_err(write_error)?
                == target.manifest_sha256
            && serde_json::from_str::<serde_json::Value>(
                &row.try_get::<String,_>("receipt_json").map_err(write_error)?
            ).map_err(|_| AuthorError::Conflict)? == receipt_value;
        if !identical {
            return Err(AuthorError::Conflict);
        }
        // clock_timestamp is wall-clock even while a transaction is waiting.
        let still_live: bool = sqlx::query_scalar(
            "SELECT locked_until>clock_timestamp() FROM chapter_import_jobs
             WHERE id=$1 AND lease_epoch=$2 AND status='running'"
        ).bind(job_id).bind(epoch).fetch_one(&mut *tx).await.map_err(write_error)?;
        if !still_live {
            return Err(AuthorError::Conflict);
        }
        tx.commit().await.map_err(write_error)
    }
}
