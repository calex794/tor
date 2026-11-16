//! DROP-04b: atomic fenced B materialization of a previously verified
//! direct PACK stage. The caller MUST independently full-GET and verify the
//! private S3 PACK and sidecar before invoking this internal-only method.
//! No HTTP route or normal S023 worker invokes this until C/G active-PACK
//! reading and rollback acceptance have been completed.
use super::{write_error, PgFreeS3Publication};
use crate::services::{
    authoring::{valid_command_key, AuthorError},
    chapter_file_import::{fingerprint, ImportedChapter, MAX_IMPORT_CHAPTERS},
};
use sha2::{Digest, Sha256};
use sqlx::Row;
use std::collections::HashSet;
use tor_contracts::drop_pack::PackPlacementReceipt;

pub struct DirectPackFinalize<'a> {
    pub job_id: i64,
    pub epoch: i64,
    pub actor: i64,
    pub book: i64,
    pub command_key: &'a str,
    pub chapters: &'a [ImportedChapter],
    pub receipt: &'a PackPlacementReceipt,
}

impl PgFreeS3Publication {
    /// In ONE transaction: immutable chapter/version/shape, verified PACK and
    /// frame generations, first active pointer, chapter publication, batch
    /// command and S023 job success. Either the entire batch exists or none.
    ///
    /// There is NO SINGLE receipt. Publication is protected by the new S349
    /// guard, which requires an exact PACK active pointer and trusted stage.
    pub async fn finish_verified_direct_pack_job(
        &self,
        request: DirectPackFinalize<'_>,
    ) -> Result<usize, AuthorError> {
        let DirectPackFinalize {
            job_id,
            epoch,
            actor,
            book,
            command_key,
            chapters,
            receipt,
        } = request;
        let target = &receipt.target;
        if job_id <= 0
            || epoch <= 0
            || actor <= 0
            || book <= 0
            || !valid_command_key(command_key)
            || chapters.is_empty()
            || chapters.len() > MAX_IMPORT_CHAPTERS
            || receipt.frames.len() != chapters.len()
            || !target.pack_key.starts_with("private/free-packs/import/")
            || target.pack_sha256 == [0; 32]
            || target.manifest_sha256 == [0; 32]
            || !(26..=16_777_216).contains(&target.pack_bytes)
            || !(1..=1_048_576).contains(&target.manifest_bytes)
            || target.seek_table_start == 0
            || target.seek_table_start >= target.pack_bytes
        {
            return Err(AuthorError::Invalid);
        }
        let approved = fingerprint(book, chapters);
        let receipt_value = serde_json::to_value(receipt).map_err(|_| AuthorError::Invalid)?;
        let mut seen = HashSet::new();
        let mut offset = 0_u64;
        for (source, frame) in chapters.iter().zip(&receipt.frames) {
            let sha: [u8; 32] = Sha256::digest(source.body.as_bytes()).into();
            if sha != frame.content_sha256
                || frame.chapter_id <= 0
                || frame.version_id <= 0
                || !seen.insert((frame.chapter_id, frame.version_id))
                || frame.frame_offset != offset
                || frame.size_bytes != source.body.len() as u64
                || !(1..=200_000).contains(&frame.size_bytes)
                || !(1..=204_096).contains(&frame.frame_bytes)
            {
                return Err(AuthorError::Conflict);
            }
            offset = offset
                .checked_add(frame.frame_bytes)
                .ok_or(AuthorError::Invalid)?;
        }
        if offset != target.seek_table_start {
            return Err(AuthorError::Conflict);
        }

        let mut tx = self.pool.begin().await.map_err(write_error)?;
        let job = sqlx::query(
            "SELECT actor_user_id,book_id,command_key,status,lease_epoch,
                    approved_fingerprint,expected_chapter_count,
                    locked_until>clock_timestamp() AS lease_valid
             FROM chapter_import_jobs WHERE id=$1 FOR UPDATE",
        )
        .bind(job_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?
        .ok_or(AuthorError::Conflict)?;
        let status: String = job.try_get("status").map_err(write_error)?;
        if job
            .try_get::<i64, _>("actor_user_id")
            .map_err(write_error)?
            != actor
            || job.try_get::<i64, _>("book_id").map_err(write_error)? != book
            || job
                .try_get::<String, _>("command_key")
                .map_err(write_error)?
                != command_key
            || job.try_get::<i64, _>("lease_epoch").map_err(write_error)? != epoch
            || job
                .try_get::<Vec<u8>, _>("approved_fingerprint")
                .map_err(write_error)?
                != approved
            || job
                .try_get::<i32, _>("expected_chapter_count")
                .map_err(write_error)?
                != chapters.len() as i32
        {
            return Err(AuthorError::Conflict);
        }
        let stage = sqlx::query(
            "SELECT actor_user_id,book_id,approved_fingerprint,chapter_count,
                    bucket,object_key,pack_sha256,manifest_sha256,
                    receipt::text AS receipt_json
             FROM free_pack_direct_import_stages WHERE job_id=$1 FOR SHARE",
        )
        .bind(job_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?
        .ok_or(AuthorError::Conflict)?;
        let staged_json: String = stage.try_get("receipt_json").map_err(write_error)?;
        let saved: serde_json::Value =
            serde_json::from_str(&staged_json).map_err(|_| AuthorError::Conflict)?;
        if saved != receipt_value
            || stage
                .try_get::<i64, _>("actor_user_id")
                .map_err(write_error)?
                != actor
            || stage.try_get::<i64, _>("book_id").map_err(write_error)? != book
            || stage
                .try_get::<Vec<u8>, _>("approved_fingerprint")
                .map_err(write_error)?
                != approved
            || stage
                .try_get::<i32, _>("chapter_count")
                .map_err(write_error)?
                != chapters.len() as i32
            || stage.try_get::<String, _>("bucket").map_err(write_error)? != target.bucket
            || stage
                .try_get::<String, _>("object_key")
                .map_err(write_error)?
                != target.pack_key
            || stage
                .try_get::<Vec<u8>, _>("pack_sha256")
                .map_err(write_error)?
                != target.pack_sha256
            || stage
                .try_get::<Vec<u8>, _>("manifest_sha256")
                .map_err(write_error)?
                != target.manifest_sha256
        {
            return Err(AuthorError::Conflict);
        }
        if status == "succeeded" {
            // Lost PG COMMIT acknowledgement. Return the same successful
            // batch only if its complete immutable publication actually exists.
            let attached: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM free_edition_storage_generations g
                 JOIN free_edition_active_storage a
                   ON a.chapter_id=g.chapter_id AND a.version_id=g.version_id
                  AND a.generation=g.generation
                 JOIN free_storage_packs pack ON pack.id=g.pack_id
                 JOIN chapters c ON c.id=g.chapter_id AND c.book_id=$2
                 JOIN chapter_publications pub
                   ON pub.chapter_id=g.chapter_id AND pub.version_id=g.version_id
                 WHERE pack.object_key=$1 AND g.placement_kind='pack'",
            )
            .bind(&target.pack_key)
            .bind(book)
            .fetch_one(&mut *tx)
            .await
            .map_err(write_error)?;
            let batch: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM chapter_import_batches
                 WHERE user_id=$1 AND command_key=$2 AND book_id=$3
                   AND content_sha256=$4 AND chapter_count=$5)",
            )
            .bind(actor)
            .bind(command_key)
            .bind(book)
            .bind(approved.as_slice())
            .bind(chapters.len() as i32)
            .fetch_one(&mut *tx)
            .await
            .map_err(write_error)?;
            return if attached == chapters.len() as i64 && batch {
                Ok(chapters.len())
            } else {
                Err(AuthorError::Conflict)
            };
        }
        if status != "running" || !job.try_get::<bool, _>("lease_valid").map_err(write_error)? {
            return Err(AuthorError::Conflict);
        }
        let owner: Option<i64> = sqlx::query_scalar(
            "SELECT b.id FROM books b JOIN users u ON u.id=b.owner_user_id
             WHERE b.id=$1 AND b.owner_user_id=$2 AND b.visibility='draft'
               AND u.account_state='active' AND NOT b.admin_hold AND NOT b.is_deleted
             FOR NO KEY UPDATE OF b,u",
        )
        .bind(book)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
        if owner.is_none() {
            return Err(AuthorError::Conflict);
        }
        let rows = sqlx::query(
            "SELECT chapter_index,chapter_id,version_id,chapter_no,title,
                    batch_sha256,content_sha256,size_bytes
             FROM free_chapter_import_reservations
             WHERE user_id=$1 AND book_id=$2 AND command_key=$3
             ORDER BY chapter_index FOR NO KEY UPDATE",
        )
        .bind(actor)
        .bind(book)
        .bind(command_key)
        .fetch_all(&mut *tx)
        .await
        .map_err(write_error)?;
        if rows.len() != chapters.len() {
            return Err(AuthorError::Conflict);
        }
        let mut numbers = Vec::with_capacity(chapters.len());
        let mut versions = Vec::with_capacity(chapters.len());
        for (i, ((row, chapter), frame)) in
            rows.iter().zip(chapters).zip(&receipt.frames).enumerate()
        {
            let hash: [u8; 32] = Sha256::digest(chapter.body.as_bytes()).into();
            if row
                .try_get::<i32, _>("chapter_index")
                .map_err(write_error)?
                != i as i32
                || row.try_get::<i64, _>("chapter_id").map_err(write_error)? != frame.chapter_id
                || row.try_get::<i64, _>("version_id").map_err(write_error)? != frame.version_id
                || row.try_get::<i32, _>("chapter_no").map_err(write_error)? != chapter.number
                || row.try_get::<String, _>("title").map_err(write_error)? != chapter.title
                || row
                    .try_get::<Vec<u8>, _>("batch_sha256")
                    .map_err(write_error)?
                    != approved
                || row
                    .try_get::<Vec<u8>, _>("content_sha256")
                    .map_err(write_error)?
                    != hash
                || hash != frame.content_sha256
                || row.try_get::<i64, _>("size_bytes").map_err(write_error)?
                    != chapter.body.len() as i64
            {
                return Err(AuthorError::Conflict);
            }
            numbers.push(chapter.number);
            versions.push(frame.version_id);
        }
        let mixed: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM free_chapter_import_uploads
             WHERE version_id=ANY($1::bigint[]))",
        )
        .bind(&versions)
        .fetch_one(&mut *tx)
        .await
        .map_err(write_error)?;
        let reused: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM chapters
             WHERE book_id=$1 AND chapter_no=ANY($2::int[]))",
        )
        .bind(book)
        .bind(&numbers)
        .fetch_one(&mut *tx)
        .await
        .map_err(write_error)?;
        if mixed || reused {
            return Err(AuthorError::Conflict);
        }
        let pack_id: i64 = sqlx::query_scalar(
            "INSERT INTO free_storage_packs
             (bucket,object_key,pack_sha256,manifest_key,manifest_sha256,
              manifest_bytes,pack_bytes,seek_table_start)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id",
        )
        .bind(&target.bucket)
        .bind(&target.pack_key)
        .bind(target.pack_sha256.as_slice())
        .bind(&target.manifest_key)
        .bind(target.manifest_sha256.as_slice())
        .bind(target.manifest_bytes as i64)
        .bind(target.pack_bytes as i64)
        .bind(target.seek_table_start as i64)
        .fetch_one(&mut *tx)
        .await
        .map_err(write_error)?;
        for (source, frame) in chapters.iter().zip(&receipt.frames) {
            sqlx::query(
                "INSERT INTO chapters
                 (id,book_id,chapter_no,title,visibility,access_level,price_amount,edit_revision)
                 VALUES($1,$2,$3,$4,'draft','free',0,1)",
            )
            .bind(frame.chapter_id)
            .bind(book)
            .bind(source.number)
            .bind(&source.title)
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
            sqlx::query(
                "INSERT INTO chapter_versions(id,chapter_id,version_no,content_sha256)
                 VALUES($1,$2,1,$3)",
            )
            .bind(frame.version_id)
            .bind(frame.chapter_id)
            .bind(frame.content_sha256.as_slice())
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
            let shape = sqlx::query(
                "INSERT INTO chapter_version_content_shapes
                 (chapter_id,version_id,content_sha256,block_utf16_lengths,
                  last_readable_block_index,last_readable_char_offset)
                 SELECT chapter_id,version_id,content_sha256,block_utf16_lengths,
                        last_readable_block_index,last_readable_char_offset
                 FROM free_chapter_import_reservations WHERE version_id=$1",
            )
            .bind(frame.version_id)
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
            if shape.rows_affected() != 1 {
                return Err(AuthorError::Conflict);
            }
            sqlx::query(
                "INSERT INTO free_edition_storage_generations
                 (chapter_id,version_id,generation,placement_kind,content_sha256,
                  size_bytes,pack_id,frame_offset,frame_bytes)
                 VALUES($1,$2,1,'pack',$3,$4,$5,$6,$7)",
            )
            .bind(frame.chapter_id)
            .bind(frame.version_id)
            .bind(frame.content_sha256.as_slice())
            .bind(frame.size_bytes as i64)
            .bind(pack_id)
            .bind(frame.frame_offset as i64)
            .bind(frame.frame_bytes as i64)
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
            let promoted: bool =
                sqlx::query_scalar("SELECT promote_free_edition_storage($1,$2,0,1)")
                    .bind(frame.chapter_id)
                    .bind(frame.version_id)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(write_error)?;
            if !promoted {
                return Err(AuthorError::Conflict);
            }
            // S349 publication trigger is preserved: it validates the exact
            // staged PACK active pointer in this same uncommitted transaction.
            sqlx::query(
                "INSERT INTO chapter_publications(chapter_id,version_id)
                 VALUES($1,$2)",
            )
            .bind(frame.chapter_id)
            .bind(frame.version_id)
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
            sqlx::query(
                "UPDATE chapters SET visibility='public' WHERE id=$1 AND visibility='draft'",
            )
            .bind(frame.chapter_id)
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
        }
        sqlx::query(
            "INSERT INTO chapter_import_batches
             (user_id,command_key,book_id,content_sha256,chapter_count)
             VALUES($1,$2,$3,$4,$5)",
        )
        .bind(actor)
        .bind(command_key)
        .bind(book)
        .bind(approved.as_slice())
        .bind(chapters.len() as i32)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        let finished = sqlx::query(
            "UPDATE chapter_import_jobs SET status='succeeded',locked_until=NULL,
                    saved_count=$3,last_error_code=NULL,updated_at=clock_timestamp(),
                    finished_at=clock_timestamp()
             WHERE id=$1 AND lease_epoch=$2 AND status='running'
               AND locked_until>clock_timestamp()",
        )
        .bind(job_id)
        .bind(epoch)
        .bind(chapters.len() as i32)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        if finished.rows_affected() != 1 {
            return Err(AuthorError::Conflict);
        }
        tx.commit().await.map_err(write_error)?;
        Ok(chapters.len())
    }
}
