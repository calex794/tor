//! DROP-R4a: B-owned, lease-fenced PG candidate attachment.
//! No B->G cross-feature dependency: the only input is the neutral I contract.
//! Trusted orchestration must obtain that descriptor from the G full S3 proof,
//! NEVER directly from an HTTP actor or an unverified manifest.
//! No S3 I/O occurs inside the B transaction; no active reader is switched.

use crate::services::authoring::AuthorError;
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::collections::HashSet;
use tor_contracts::drop_pack::PackPlacementReceipt;

#[derive(Clone)]
pub struct PgFreePackCatalog {
    pool: PgPool,
}

impl PgFreePackCatalog {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Preapproved complete ordered physical inventory, including expected
    /// immutable B edition IDs, hashes and all pack/sidecar contents digests.
    /// The B scheduler computes this *before* any G upload or proof.
    pub fn fingerprint(receipt: &PackPlacementReceipt) -> [u8; 32] {
        let mut hash = Sha256::new();
        fn part(hash: &mut Sha256, source: &[u8]) {
            hash.update((source.len() as u64).to_be_bytes());
            hash.update(source);
        }
        let target = &receipt.target;
        part(&mut hash, b"tor-drop-r4a-approved-pack-fingerprint-v1");
        part(&mut hash, target.bucket.as_bytes());
        part(&mut hash, target.pack_key.as_bytes());
        part(&mut hash, target.manifest_key.as_bytes());
        part(&mut hash, &target.pack_sha256);
        part(&mut hash, &target.manifest_sha256);
        for n in [
            target.pack_bytes,
            target.manifest_bytes,
            target.seek_table_start,
        ] {
            part(&mut hash, &n.to_be_bytes());
        }
        part(&mut hash, &(receipt.frames.len() as u64).to_be_bytes());
        for edition in &receipt.frames {
            part(&mut hash, &edition.chapter_id.to_be_bytes());
            part(&mut hash, &edition.version_id.to_be_bytes());
            part(&mut hash, &edition.content_sha256);
            part(&mut hash, &edition.size_bytes.to_be_bytes());
            part(&mut hash, &edition.frame_offset.to_be_bytes());
            part(&mut hash, &edition.frame_bytes.to_be_bytes());
        }
        hash.finalize().into()
    }

    /// Attach exactly one verified pack's candidate generations as one PG
    /// transaction. The source receipt is a *trusted-in-process* descriptor;
    /// B does not independently verify remote S3 bytes. A separate G worker
    /// must prove them before calling this method through the application.
    pub async fn finish_pack_job(
        &self,
        job_id: i64,
        expected_epoch: i64,
        receipt: &PackPlacementReceipt,
    ) -> Result<i64, AuthorError> {
        let target = &receipt.target;
        let frames = &receipt.frames;
        if job_id <= 0 || expected_epoch <= 0 || frames.is_empty() || frames.len() > 4096 {
            return Err(AuthorError::Invalid);
        }
        // Fail early on obviously forged/mismatched source descriptors even
        // before B's authoritative PostgreSQL validation and R2 SQL guards.
        if target.pack_sha256 == [0; 32]
            || target.manifest_sha256 == [0; 32]
            || !(26..=16 * 1024 * 1024).contains(&target.pack_bytes)
            || !(1..=1024 * 1024).contains(&target.manifest_bytes)
            || !(1..target.pack_bytes).contains(&target.seek_table_start)
            || !target.pack_key.starts_with("private/free-packs/")
            || !target.manifest_key.ends_with(".manifest.json")
        {
            return Err(AuthorError::Invalid);
        }
        let mut seen = HashSet::with_capacity(frames.len());
        for frame in frames {
            if frame.chapter_id <= 0
                || frame.version_id <= 0
                || frame.content_sha256 == [0; 32]
                || !(1..=200_000).contains(&frame.size_bytes)
                || !(1..=204_096).contains(&frame.frame_bytes)
                || frame
                    .frame_offset
                    .checked_add(frame.frame_bytes)
                    .is_none_or(|end| end > target.seek_table_start)
                || !seen.insert((frame.chapter_id, frame.version_id))
            {
                return Err(AuthorError::Invalid);
            }
        }
        let expected_fingerprint = Self::fingerprint(receipt);
        let mut tx = self.pool.begin().await.map_err(write_error)?;
        let row = sqlx::query(
            "SELECT actor_user_id,book_id,status,lease_epoch,
                    approved_fingerprint,expected_chapter_count,pack_id,
                    COALESCE(locked_until>clock_timestamp(),false) AS lease_valid
             FROM free_pack_jobs WHERE id=$1 FOR UPDATE",
        )
        .bind(job_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?
        .ok_or(AuthorError::Conflict)?;
        let actor: i64 = row.try_get("actor_user_id").map_err(write_error)?;
        let book: i64 = row.try_get("book_id").map_err(write_error)?;
        let status: String = row.try_get("status").map_err(write_error)?;
        let epoch: i64 = row.try_get("lease_epoch").map_err(write_error)?;
        let fingerprint: Vec<u8> = row.try_get("approved_fingerprint").map_err(write_error)?;
        let count: i32 = row.try_get("expected_chapter_count").map_err(write_error)?;
        let pack_id: Option<i64> = row.try_get("pack_id").map_err(write_error)?;
        let lease_valid: bool = row.try_get("lease_valid").map_err(write_error)?;
        if epoch != expected_epoch
            || fingerprint != expected_fingerprint
            || count as usize != frames.len()
        {
            return Err(AuthorError::Conflict);
        }
        if status == "succeeded" {
            // Lost-commit-ACK retry; no second physical generation.
            return pack_id.ok_or(AuthorError::Conflict);
        }
        if status != "running" || !lease_valid || actor <= 0 || book <= 0 {
            return Err(AuthorError::Conflict);
        }

        let owner: Option<i64> = sqlx::query_scalar(
            "SELECT b.owner_user_id FROM books b JOIN users u ON u.id=b.owner_user_id
             WHERE b.id=$1 AND b.owner_user_id=$2 AND u.account_state='active'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND b.visibility IN ('draft','public')
             FOR NO KEY UPDATE OF b, u",
        )
        .bind(book)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
        if owner.is_none() {
            return Err(AuthorError::Conflict);
        }
        let mut ordered: Vec<_> = frames.iter().collect();
        ordered.sort_unstable_by_key(|f| (f.chapter_id, f.version_id));
        for frame in &ordered {
            let exact: Option<i64> = sqlx::query_scalar(
                "SELECT c.id FROM chapters c
                 JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=$3
                 JOIN free_chapter_storage_objects s
                   ON s.chapter_id=c.id AND s.version_id=v.id
                 WHERE c.id=$1 AND c.book_id=$2 AND c.access_level='free'
                   AND c.price_amount=0 AND NOT c.is_deleted AND NOT c.admin_hold
                   AND v.content_sha256=$4 AND s.sha256=$4 AND s.size_bytes=$5
                 FOR NO KEY UPDATE OF c",
            )
            .bind(frame.chapter_id)
            .bind(book)
            .bind(frame.version_id)
            .bind(frame.content_sha256.as_slice())
            .bind(frame.size_bytes as i64)
            .fetch_optional(&mut *tx)
            .await
            .map_err(write_error)?;
            if exact.is_none() {
                return Err(AuthorError::Conflict);
            }
        }

        let pack_id: i64 = sqlx::query_scalar(
            "INSERT INTO free_storage_packs
                (bucket,object_key,manifest_key,pack_sha256,manifest_sha256,
                 manifest_bytes,pack_bytes,seek_table_start)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8) RETURNING id",
        )
        .bind(&target.bucket)
        .bind(&target.pack_key)
        .bind(&target.manifest_key)
        .bind(target.pack_sha256.as_slice())
        .bind(target.manifest_sha256.as_slice())
        .bind(target.manifest_bytes as i64)
        .bind(target.pack_bytes as i64)
        .bind(target.seek_table_start as i64)
        .fetch_one(&mut *tx)
        .await
        .map_err(write_error)?;
        for frame in ordered {
            let next: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(generation),0)+1
                 FROM free_edition_storage_generations
                 WHERE chapter_id=$1 AND version_id=$2",
            )
            .bind(frame.chapter_id)
            .bind(frame.version_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(write_error)?;
            sqlx::query(
                "INSERT INTO free_edition_storage_generations
                    (chapter_id,version_id,generation,placement_kind,content_sha256,
                     size_bytes,pack_id,frame_offset,frame_bytes)
                 VALUES($1,$2,$3,'pack',$4,$5,$6,$7,$8)",
            )
            .bind(frame.chapter_id)
            .bind(frame.version_id)
            .bind(next)
            .bind(frame.content_sha256.as_slice())
            .bind(frame.size_bytes as i64)
            .bind(pack_id)
            .bind(frame.frame_offset as i64)
            .bind(frame.frame_bytes as i64)
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
        }
        let updated = sqlx::query(
            "UPDATE free_pack_jobs SET status='succeeded',pack_id=$3,
                locked_until=NULL,finished_at=now(),updated_at=now()
             WHERE id=$1 AND lease_epoch=$2 AND status='running'
               AND locked_until>clock_timestamp()",
        )
        .bind(job_id)
        .bind(expected_epoch)
        .bind(pack_id)
        .execute(&mut *tx)
        .await
        .map_err(write_error)?;
        if updated.rows_affected() != 1 {
            return Err(AuthorError::Conflict);
        }
        tx.commit().await.map_err(write_error)?;
        Ok(pack_id)
    }
}

fn write_error(error: sqlx::Error) -> AuthorError {
    if let sqlx::Error::Database(ref db) = error {
        if matches!(db.code().as_deref(), Some("23503" | "23505" | "23514")) {
            return AuthorError::Conflict;
        }
    }
    AuthorError::Unavailable
}
