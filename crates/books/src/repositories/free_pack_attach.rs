//! DROP-R4a: B-owned, lease-fenced PG attachment of a G-verified PACK.
//!
//! This is an **inert candidate catalog**. It never updates
//! free_edition_active_storage, chapter_publications or reader permissions.
//! No S3 network I/O happens while a PostgreSQL transaction is open.
//! A production caller must first obtain VerifiedPack from G's independent
//! full S3 GET and compare against a previously approved B inventory.

use sha2::{Digest, Sha256};
use sqlx::{PgPool, Row};
use std::collections::HashSet;
use tor_storage::services::{
    free_chapter::FreeChapterObject,
    free_pack_proof::{PackProofTarget, VerifiedPack},
};

use crate::services::authoring::AuthorError;

#[derive(Clone)]
pub struct PgFreePackCatalog {
    pool: PgPool,
}

impl PgFreePackCatalog {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// A stable, v1-fenced fingerprint for a *preapproved* target and its full
    /// ordered inventory. B should persist it before G uploads anything; the
    /// exact same digest must be reproduced from G's opaque verification.
    pub fn fingerprint(
        target: &PackProofTarget,
        editions: &[FreeChapterObject],
    ) -> [u8; 32] {
        let mut hash = Sha256::new();
        fn part(hash: &mut Sha256, source: &[u8]) {
            hash.update((source.len() as u64).to_be_bytes());
            hash.update(source);
        }
        part(&mut hash, b"tor-drop-r4a-approved-pack-fingerprint-v1");
        part(&mut hash, target.bucket.as_bytes());
        part(&mut hash, target.pack_key.as_bytes());
        part(&mut hash, target.manifest_key.as_bytes());
        part(&mut hash, &target.pack_sha256);
        part(&mut hash, &target.manifest_sha256);
        for n in [target.pack_bytes, target.manifest_bytes, target.seek_table_start] {
            part(&mut hash, &n.to_be_bytes());
        }
        part(&mut hash, &(editions.len() as u64).to_be_bytes());
        for edition in editions {
            part(&mut hash, &edition.chapter_id().to_be_bytes());
            part(&mut hash, &edition.version_id().to_be_bytes());
            part(&mut hash, edition.sha256());
            part(&mut hash, &edition.size_bytes().to_be_bytes());
        }
        hash.finalize().into()
    }

    /// Attach all candidate generations atomically iff the current job has
    /// a live, exact epoch and fingerprint, and every immutable B edition plus
    /// original S349 SINGLE receipt matches G's fully verified S3 bytes.
    ///
    /// A successful retry after a lost PG commit ACK returns the same pack_id.
    /// An old epoch, expired lease, author hold, missing/paid edition, foreign
    /// book, stale source hash or any broken frame rolls back the entire tx.
    ///
    /// This function does *not* promote a generation: that requires a future
    /// independently authorized C gateway and an explicit cutover protocol.
    pub async fn finish_pack_job(
        &self,
        job_id: i64,
        expected_epoch: i64,
        proof: &VerifiedPack,
    ) -> Result<i64, AuthorError> {
        let target = proof.target();
        let frames = proof.frames();
        if job_id <= 0 || expected_epoch <= 0 || frames.is_empty() || frames.len() > 4096 {
            return Err(AuthorError::Invalid);
        }
        // A type from the G verifier is opaque, but assert all frame metadata
        // equal the trusted whole-object proof target as an extra boundary.
        let mut unique = HashSet::with_capacity(frames.len());
        for frame in frames {
            let m = frame.metadata();
            if m.bucket != target.bucket
                || m.key != target.pack_key
                || m.pack_bytes != target.pack_bytes
                || m.seek_table_start != target.seek_table_start
                || m.pack_sha256 != target.pack_sha256
                || m.manifest_sha256 != target.manifest_sha256
                || !unique.insert((frame.edition().chapter_id(), frame.edition().version_id()))
            {
                return Err(AuthorError::Invalid);
            }
        }
        let editions: Vec<_> = frames.iter().map(|f| f.edition().clone()).collect();
        let expected_fingerprint = Self::fingerprint(target, &editions);
        let mut tx = self.pool.begin().await.map_err(write_error)?;
        let row = sqlx::query(
            "SELECT actor_user_id,book_id,status,lease_epoch,
                    approved_fingerprint,expected_chapter_count,pack_id,
                    COALESCE(locked_until>now(),false) AS lease_valid
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
        // Lost-ACK idempotence: never create new immutable generations.
        if status == "succeeded" {
            return pack_id.ok_or(AuthorError::Conflict);
        }
        if status != "running" || !lease_valid || actor <= 0 || book <= 0 {
            return Err(AuthorError::Conflict);
        }
        let owner: Option<i64> = sqlx::query_scalar(
            "SELECT owner_user_id FROM books
             WHERE id=$1 AND owner_user_id=$2 AND NOT admin_hold AND NOT is_deleted
               AND visibility IN ('draft','public')
             FOR NO KEY UPDATE",
        )
        .bind(book)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?;
        if owner.is_none() {
            return Err(AuthorError::Conflict);
        }
        // Lock chapter state in canonical order, without a per-frame connection
        // lock: the whole job and its generation writes are one B transaction.
        let mut ordered: Vec<_> = frames.iter().collect();
        ordered.sort_unstable_by_key(|f| (f.edition().chapter_id(), f.edition().version_id()));
        for frame in &ordered {
            let edition = frame.edition();
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
            .bind(edition.chapter_id())
            .bind(book)
            .bind(edition.version_id())
            .bind(edition.sha256().as_slice())
            .bind(edition.size_bytes() as i64)
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
            let edition = frame.edition();
            let next: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(generation),0)+1
                 FROM free_edition_storage_generations
                 WHERE chapter_id=$1 AND version_id=$2",
            )
            .bind(edition.chapter_id())
            .bind(edition.version_id())
            .fetch_one(&mut *tx)
            .await
            .map_err(write_error)?;
            sqlx::query(
                "INSERT INTO free_edition_storage_generations
                    (chapter_id,version_id,generation,placement_kind,content_sha256,
                     size_bytes,pack_id,frame_offset,frame_bytes)
                 VALUES($1,$2,$3,'pack',$4,$5,$6,$7,$8)",
            )
            .bind(edition.chapter_id())
            .bind(edition.version_id())
            .bind(next)
            .bind(edition.sha256().as_slice())
            .bind(edition.size_bytes() as i64)
            .bind(pack_id)
            .bind(frame.metadata().frame_offset as i64)
            .bind(frame.metadata().frame_bytes as i64)
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
        }

        let updated = sqlx::query(
            "UPDATE free_pack_jobs SET status='succeeded',pack_id=$3,
                locked_until=NULL,finished_at=now(),updated_at=now()
             WHERE id=$1 AND lease_epoch=$2 AND status='running'
               AND locked_until>now()",
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
