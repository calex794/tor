//! DROP-05a: atomic, internal-only activation of a verified R4a PACK.
//!
//! This catalog operation NEVER performs S3 I/O and MUST NOT be called from
//! HTTP. Its caller must re-prove the entire immutable PACK and manifest using
//! G immediately before passing the exact receipt. It is not wired to startup.
//! No historic catalogue backfill exists for greenfield TOR.
use super::free_pack_attach::PgFreePackCatalog;
use crate::services::authoring::AuthorError;
use sqlx::{postgres::PgRow, Row};
use std::collections::HashSet;
use tor_contracts::drop_pack::PackPlacementReceipt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackActivation {
    Activated,
    AlreadyActive,
}

fn db_error(error: sqlx::Error) -> AuthorError {
    if let sqlx::Error::Database(ref db) = error {
        if matches!(db.code().as_deref(), Some("23503" | "23505" | "23514")) {
            return AuthorError::Conflict;
        }
    }
    AuthorError::Unavailable
}

fn column<T>(row: &PgRow, name: &str) -> Result<T, AuthorError>
where
    for<'r> T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get(name).map_err(|_| AuthorError::Unavailable)
}

impl PgFreePackCatalog {
    /// Atomically switch *every* member of one succeeded, exact, independently
    /// re-proved R4a PACK from its current SINGLE to its already immutable PACK
    /// generation. Any stale/partial/concurrent state fails closed as a whole.
    ///
    /// R4a job epoch is immutable after success; a caller cannot replay an old
    /// claimed epoch from a replaced worker. An identical successful replay
    /// returns AlreadyActive, without advancing any generation or doing S3 I/O.
    ///
    /// This is the B transaction only. It deliberately is not a public method
    /// of the web application and is not scheduled until G reproof exists.
    pub async fn activate_verified_pack(
        &self,
        job_id: i64,
        expected_epoch: i64,
        verified: &PackPlacementReceipt,
    ) -> Result<PackActivation, AuthorError> {
        if job_id <= 0
            || expected_epoch <= 0
            || verified.frames.is_empty()
            || verified.frames.len() > 4096
            || verified.target.pack_sha256 == [0; 32]
            || verified.target.manifest_sha256 == [0; 32]
        {
            return Err(AuthorError::Invalid);
        }
        let fingerprint = Self::fingerprint(verified);
        let mut seen = HashSet::with_capacity(verified.frames.len());
        for frame in &verified.frames {
            if frame.chapter_id <= 0
                || frame.version_id <= 0
                || frame.content_sha256 == [0; 32]
                || !(1..=200_000).contains(&frame.size_bytes)
                || !seen.insert((frame.chapter_id, frame.version_id))
            {
                return Err(AuthorError::Invalid);
            }
        }
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let job = sqlx::query(
            "SELECT actor_user_id,book_id,lease_epoch,pack_id,
                    approved_fingerprint,expected_chapter_count
             FROM free_pack_jobs WHERE id=$1 AND status='succeeded'
             FOR UPDATE",
        )
        .bind(job_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?
        .ok_or(AuthorError::Conflict)?;
        let actor: i64 = column(&job, "actor_user_id")?;
        let book: i64 = column(&job, "book_id")?;
        let epoch: i64 = column(&job, "lease_epoch")?;
        let pack_id: i64 = job
            .try_get::<Option<i64>, _>("pack_id")
            .map_err(db_error)?
            .ok_or(AuthorError::Conflict)?;
        let approved: Vec<u8> = column(&job, "approved_fingerprint")?;
        let count: i32 = column(&job, "expected_chapter_count")?;
        if epoch != expected_epoch
            || approved != fingerprint
            || count as usize != verified.frames.len()
        {
            return Err(AuthorError::Conflict);
        }

        // The committed immutable physical descriptor must match the entire
        // trusted reproof; a matching job ID alone is not sufficient.
        let pack_matches: bool = sqlx::query_scalar(
            "SELECT EXISTS(
                SELECT 1 FROM free_storage_packs p WHERE p.id=$1
                AND p.bucket=$2 AND p.object_key=$3 AND p.manifest_key=$4
                AND p.pack_sha256=$5 AND p.manifest_sha256=$6
                AND p.pack_bytes=$7 AND p.manifest_bytes=$8
                AND p.seek_table_start=$9)",
        )
        .bind(pack_id)
        .bind(&verified.target.bucket)
        .bind(&verified.target.pack_key)
        .bind(&verified.target.manifest_key)
        .bind(verified.target.pack_sha256.as_slice())
        .bind(verified.target.manifest_sha256.as_slice())
        .bind(verified.target.pack_bytes as i64)
        .bind(verified.target.manifest_bytes as i64)
        .bind(verified.target.seek_table_start as i64)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
        if !pack_matches {
            return Err(AuthorError::Conflict);
        }

        // Serialize with moderation, ownership transfer and same-book cutovers.
        let owned: Option<i64> = sqlx::query_scalar(
            "SELECT b.owner_user_id FROM books b
             JOIN users u ON u.id=b.owner_user_id
             WHERE b.id=$1 AND b.owner_user_id=$2
               AND u.account_state='active'
               AND NOT b.admin_hold AND NOT b.is_deleted
               AND b.visibility IN ('draft','public')
             FOR NO KEY UPDATE OF b,u",
        )
        .bind(book)
        .bind(actor)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?;
        if owned.is_none() {
            return Err(AuthorError::Conflict);
        }

        // Immutable membership cannot be silently dropped or enlarged by
        // sorting/replaying only the subset that happens to remain readable.
        let member_rows = sqlx::query(
            "SELECT chapter_id,version_id FROM free_pack_job_members
             WHERE job_id=$1 ORDER BY chapter_id,version_id",
        )
        .bind(job_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(db_error)?;
        let members: HashSet<(i64, i64)> = member_rows
            .iter()
            .map(|r| {
                Ok((
                    column::<i64>(r, "chapter_id")?,
                    column::<i64>(r, "version_id")?,
                ))
            })
            .collect::<Result<_, AuthorError>>()?;
        if members != seen || member_rows.len() != verified.frames.len() {
            return Err(AuthorError::Conflict);
        }

        let mut ordered: Vec<_> = verified.frames.iter().collect();
        ordered.sort_unstable_by_key(|frame| (frame.chapter_id, frame.version_id));
        let mut swaps = Vec::with_capacity(ordered.len());
        let mut already = 0_usize;

        for frame in ordered {
            // Source SINGLE receipt remains physically present. Even when the
            // active location is PACK, no deletion is authorized here.
            let source: Option<i64> = sqlx::query_scalar(
                "SELECT c.id FROM chapters c
                 JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=$3
                 JOIN free_chapter_storage_objects s
                   ON s.chapter_id=c.id AND s.version_id=v.id
                 WHERE c.id=$1 AND c.book_id=$2
                   AND c.access_level='free' AND c.price_amount=0
                   AND NOT c.is_deleted AND NOT c.admin_hold
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
            .map_err(db_error)?;
            if source.is_none() {
                return Err(AuthorError::Conflict);
            }

            let candidate: Option<i64> = sqlx::query_scalar(
                "SELECT generation FROM free_edition_storage_generations
                 WHERE chapter_id=$1 AND version_id=$2
                   AND placement_kind='pack' AND pack_id=$3
                   AND content_sha256=$4 AND size_bytes=$5
                   AND frame_offset=$6 AND frame_bytes=$7",
            )
            .bind(frame.chapter_id)
            .bind(frame.version_id)
            .bind(pack_id)
            .bind(frame.content_sha256.as_slice())
            .bind(frame.size_bytes as i64)
            .bind(frame.frame_offset as i64)
            .bind(frame.frame_bytes as i64)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?;
            let next = candidate.ok_or(AuthorError::Conflict)?;

            let active = sqlx::query(
                "SELECT a.generation,g.placement_kind,g.pack_id,
                        g.content_sha256,g.size_bytes
                 FROM free_edition_active_storage a
                 JOIN free_edition_storage_generations g
                   ON g.chapter_id=a.chapter_id AND g.version_id=a.version_id
                  AND g.generation=a.generation
                 WHERE a.chapter_id=$1 AND a.version_id=$2
                 FOR UPDATE OF a",
            )
            .bind(frame.chapter_id)
            .bind(frame.version_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(db_error)?
            .ok_or(AuthorError::Conflict)?;
            let current: i64 = column(&active, "generation")?;
            let kind: String = column(&active, "placement_kind")?;
            let active_pack: Option<i64> = column(&active, "pack_id")?;
            let active_hash: Vec<u8> = column(&active, "content_sha256")?;
            let active_size: i64 = column(&active, "size_bytes")?;
            if active_hash != frame.content_sha256 || active_size != frame.size_bytes as i64 {
                return Err(AuthorError::Conflict);
            }

            if kind == "pack" && current == next && active_pack == Some(pack_id) {
                already += 1;
            } else if kind == "single" && current < next && active_pack.is_none() {
                swaps.push((frame.chapter_id, frame.version_id, current, next));
            } else {
                return Err(AuthorError::Conflict);
            }
        }

        // A partially switched PACK is not a successful replay; even if all
        // other members are valid, the whole batch is rejected.
        if already != 0 && !swaps.is_empty() {
            return Err(AuthorError::Conflict);
        }
        if already == verified.frames.len() {
            tx.commit().await.map_err(db_error)?;
            return Ok(PackActivation::AlreadyActive);
        }
        if swaps.len() != verified.frames.len() {
            return Err(AuthorError::Conflict);
        }

        for (chapter, version, old, next) in swaps {
            let changed = sqlx::query(
                "UPDATE free_edition_active_storage
                 SET generation=$4,changed_at=clock_timestamp()
                 WHERE chapter_id=$1 AND version_id=$2 AND generation=$3",
            )
            .bind(chapter)
            .bind(version)
            .bind(old)
            .bind(next)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
            if changed.rows_affected() != 1 {
                return Err(AuthorError::Conflict);
            }
        }
        tx.commit().await.map_err(db_error)?;
        Ok(PackActivation::Activated)
    }
}
