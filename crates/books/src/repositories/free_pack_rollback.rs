//! DROP-05c: deliberate, operator-only rollback of an active FREE PACK.
//!
//! G must independently reread ALL immutable original SINGLE objects before
//! invoking this B transaction. No automatic fallback on PACK read failure.
//! No S3 DELETE, HTTP route, new grant, or publication change occurs here.
use super::free_pack_attach::PgFreePackCatalog;
use crate::services::authoring::AuthorError;
use sqlx::Row;
use std::collections::HashSet;
use tor_contracts::drop_pack::PackPlacementReceipt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PackRollback {
    RolledBack,
    AlreadyRolledBack,
}

fn write_error(error: sqlx::Error) -> AuthorError {
    if let sqlx::Error::Database(ref db) = error {
        if matches!(db.code().as_deref(), Some("23503" | "23505" | "23514")) {
            return AuthorError::Conflict;
        }
    }
    AuthorError::Unavailable
}

impl PgFreePackCatalog {
    /// Atomic rollback of all immutable editions of one previously succeeded
    /// R4a PACK. Each new SINGLE generation is strictly larger than every
    /// prior PACK/SINGLE generation; rewinding a pointer is never allowed.
    ///
    /// Only a trusted, explicit operator path which just proved all original
    /// SINGLE bytes with private S3 may call this method. A hold or deletion
    /// does not itself prevent physical recovery; readers still reauthorize
    /// after the operation. The original source receipt must remain exact.
    pub async fn rollback_verified_pack_to_single(
        &self,
        job_id: i64,
        expected_epoch: i64,
        verified: &PackPlacementReceipt,
    ) -> Result<PackRollback, AuthorError> {
        if job_id <= 0
            || expected_epoch <= 0
            || verified.frames.is_empty()
            || verified.frames.len() > 4096
            || verified.target.pack_sha256 == [0; 32]
            || verified.target.manifest_sha256 == [0; 32]
        {
            return Err(AuthorError::Invalid);
        }
        let mut member_ids = HashSet::with_capacity(verified.frames.len());
        for f in &verified.frames {
            if f.chapter_id <= 0
                || f.version_id <= 0
                || f.content_sha256 == [0; 32]
                || !(1..=200_000).contains(&f.size_bytes)
                || !member_ids.insert((f.chapter_id, f.version_id))
            {
                return Err(AuthorError::Invalid);
            }
        }
        let fingerprint = Self::fingerprint(verified);
        let mut tx = self.pool.begin().await.map_err(write_error)?;
        let job = sqlx::query(
            "SELECT actor_user_id,book_id,lease_epoch,pack_id,
                    approved_fingerprint,expected_chapter_count
             FROM free_pack_jobs WHERE id=$1 AND status='succeeded'
             FOR UPDATE",
        )
        .bind(job_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(write_error)?
        .ok_or(AuthorError::Conflict)?;
        let actor: i64 = job.try_get("actor_user_id").map_err(write_error)?;
        let book: i64 = job.try_get("book_id").map_err(write_error)?;
        let epoch: i64 = job.try_get("lease_epoch").map_err(write_error)?;
        let pack_id: i64 = job
            .try_get::<Option<i64>, _>("pack_id")
            .map_err(write_error)?
            .ok_or(AuthorError::Conflict)?;
        let approved: Vec<u8> = job.try_get("approved_fingerprint").map_err(write_error)?;
        let count: i32 = job.try_get("expected_chapter_count").map_err(write_error)?;
        if actor <= 0
            || book <= 0
            || epoch != expected_epoch
            || approved.as_slice() != fingerprint.as_slice()
            || count as usize != verified.frames.len()
        {
            return Err(AuthorError::Conflict);
        }

        // A manifest descriptor cannot be supplied by a browser or a stale
        // candidate. It must match the succeeded immutable physical object.
        let pack_matches: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM free_storage_packs p
             WHERE p.id=$1 AND p.bucket=$2 AND p.object_key=$3
               AND p.manifest_key=$4 AND p.pack_sha256=$5
               AND p.manifest_sha256=$6 AND p.pack_bytes=$7
               AND p.manifest_bytes=$8 AND p.seek_table_start=$9)",
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
        .map_err(write_error)?;
        if !pack_matches {
            return Err(AuthorError::Conflict);
        }

        // Physical rescue must not change author/purchaser entitlements.
        // Book hold/deletion can stay in effect, so no new reader grant is made.
        let book_exists: Option<i64> =
            sqlx::query_scalar("SELECT id FROM books WHERE id=$1 FOR NO KEY UPDATE")
                .bind(book)
                .fetch_optional(&mut *tx)
                .await
                .map_err(write_error)?;
        if book_exists.is_none() {
            return Err(AuthorError::Conflict);
        }

        let actual = sqlx::query(
            "SELECT chapter_id,version_id FROM free_pack_job_members
             WHERE job_id=$1 ORDER BY chapter_id,version_id",
        )
        .bind(job_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(write_error)?;
        let actual_ids = actual
            .iter()
            .map(|row| {
                Ok((
                    row.try_get::<i64, _>("chapter_id").map_err(write_error)?,
                    row.try_get::<i64, _>("version_id").map_err(write_error)?,
                ))
            })
            .collect::<Result<HashSet<_>, AuthorError>>()?;
        if actual.len() != member_ids.len() || actual_ids != member_ids {
            return Err(AuthorError::Conflict);
        }

        let mut frames: Vec<_> = verified.frames.iter().collect();
        frames.sort_unstable_by_key(|f| (f.chapter_id, f.version_id));
        let mut rollback_members = Vec::with_capacity(frames.len());
        let mut already = 0_usize;

        for f in frames {
            let single_exists: Option<i64> = sqlx::query_scalar(
                "SELECT c.id FROM chapters c
                 JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=$3
                 JOIN free_chapter_storage_objects s
                   ON s.chapter_id=c.id AND s.version_id=v.id
                 WHERE c.id=$1 AND c.book_id=$2 AND c.access_level='free'
                   AND c.price_amount=0 AND v.content_sha256=$4
                   AND s.sha256=$4 AND s.size_bytes=$5
                   AND s.bucket=$6
                   AND s.object_key=('private/free-chapters/' || c.id || '/' || v.id || '.md')
                 FOR NO KEY UPDATE OF c",
            )
            .bind(f.chapter_id)
            .bind(book)
            .bind(f.version_id)
            .bind(f.content_sha256.as_slice())
            .bind(f.size_bytes as i64)
            .bind(&verified.target.bucket)
            .fetch_optional(&mut *tx)
            .await
            .map_err(write_error)?;
            if single_exists.is_none() {
                return Err(AuthorError::Conflict);
            }
            let pack_generation: i64 = sqlx::query_scalar(
                "SELECT generation FROM free_edition_storage_generations
                 WHERE chapter_id=$1 AND version_id=$2 AND pack_id=$3
                   AND placement_kind='pack' AND content_sha256=$4 AND size_bytes=$5
                   AND frame_offset=$6 AND frame_bytes=$7",
            )
            .bind(f.chapter_id)
            .bind(f.version_id)
            .bind(pack_id)
            .bind(f.content_sha256.as_slice())
            .bind(f.size_bytes as i64)
            .bind(f.frame_offset as i64)
            .bind(f.frame_bytes as i64)
            .fetch_optional(&mut *tx)
            .await
            .map_err(write_error)?
            .ok_or(AuthorError::Conflict)?;
            let active = sqlx::query(
                "SELECT a.generation,g.placement_kind,g.pack_id,
                        g.content_sha256,g.size_bytes
                 FROM free_edition_active_storage a
                 JOIN free_edition_storage_generations g
                   ON g.chapter_id=a.chapter_id AND g.version_id=a.version_id
                  AND g.generation=a.generation
                 WHERE a.chapter_id=$1 AND a.version_id=$2 FOR UPDATE OF a",
            )
            .bind(f.chapter_id)
            .bind(f.version_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(write_error)?
            .ok_or(AuthorError::Conflict)?;
            let current: i64 = active.try_get("generation").map_err(write_error)?;
            let kind: String = active.try_get("placement_kind").map_err(write_error)?;
            let active_pack: Option<i64> = active.try_get("pack_id").map_err(write_error)?;
            let sha: Vec<u8> = active.try_get("content_sha256").map_err(write_error)?;
            let size: i64 = active.try_get("size_bytes").map_err(write_error)?;
            if sha.as_slice() != f.content_sha256.as_slice() || size != f.size_bytes as i64 {
                return Err(AuthorError::Conflict);
            }
            if current == pack_generation && kind == "pack" && active_pack == Some(pack_id) {
                rollback_members.push((
                    f.chapter_id,
                    f.version_id,
                    current,
                    f.content_sha256,
                    size,
                ));
            } else if current > pack_generation && kind == "single" && active_pack.is_none() {
                // An exact replay after a lost PG commit ACK is read-only.
                already += 1;
            } else {
                return Err(AuthorError::Conflict);
            }
        }
        if already != 0 && !rollback_members.is_empty() {
            return Err(AuthorError::Conflict);
        }
        if already == verified.frames.len() {
            tx.commit().await.map_err(write_error)?;
            return Ok(PackRollback::AlreadyRolledBack);
        }
        if rollback_members.len() != verified.frames.len() {
            return Err(AuthorError::Conflict);
        }
        for (chapter, version, expected, sha, size) in rollback_members {
            let next: i64 = sqlx::query_scalar(
                "SELECT COALESCE(MAX(generation),0)+1
                 FROM free_edition_storage_generations
                 WHERE chapter_id=$1 AND version_id=$2",
            )
            .bind(chapter)
            .bind(version)
            .fetch_one(&mut *tx)
            .await
            .map_err(write_error)?;
            sqlx::query(
                "INSERT INTO free_edition_storage_generations
                   (chapter_id,version_id,generation,placement_kind,content_sha256,size_bytes)
                 VALUES($1,$2,$3,'single',$4,$5)",
            )
            .bind(chapter)
            .bind(version)
            .bind(next)
            .bind(sha.as_slice())
            .bind(size)
            .execute(&mut *tx)
            .await
            .map_err(write_error)?;
            let moved: bool =
                sqlx::query_scalar("SELECT promote_free_edition_storage($1,$2,$3,$4)")
                    .bind(chapter)
                    .bind(version)
                    .bind(expected)
                    .bind(next)
                    .fetch_one(&mut *tx)
                    .await
                    .map_err(write_error)?;
            if !moved {
                return Err(AuthorError::Conflict);
            }
        }
        tx.commit().await.map_err(write_error)?;
        Ok(PackRollback::RolledBack)
    }
}
