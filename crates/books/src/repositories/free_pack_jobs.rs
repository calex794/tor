//! DROP-R4b1: private repack queue. Candidates are never enabled for readers.
//! The spec is frozen at scheduling time; a worker must independently full-GET
//! and verify both S3 objects before B can attach the candidate generations.
use super::free_pack_attach::PgFreePackCatalog;
use crate::services::authoring::AuthorError;
use sqlx::Row;
use tor_contracts::drop_pack::PackPlacementReceipt;

#[derive(Clone, Debug)]
pub struct PackJobClaim {
    pub job_id: i64,
    pub epoch: i64,
    pub actor_user_id: i64,
    pub book_id: i64,
    pub receipt: PackPlacementReceipt,
}

impl PgFreePackCatalog {
    /// Internal scheduling only: callers must be trusted pack builders,
    /// never an HTTP payload. Immutable source editions are checked again by
    /// finish_pack_job before committing anything.
    pub async fn schedule_pack_job(
        &self,
        actor: i64,
        book: i64,
        receipt: &PackPlacementReceipt,
    ) -> Result<i64, AuthorError> {
        if actor <= 0
            || book <= 0
            || receipt.frames.is_empty()
            || receipt.frames.len() > 4096
            || receipt.target.pack_sha256 == [0; 32]
            || receipt.target.manifest_sha256 == [0; 32]
            || receipt.target.pack_bytes < 26
            || receipt.target.pack_bytes > 16 * 1024 * 1024
            || receipt.target.manifest_bytes == 0
            || receipt.target.manifest_bytes > 1024 * 1024
            || !receipt.target.pack_key.starts_with("private/free-packs/")
            || receipt.target.manifest_key
                != receipt.target.pack_key.replace(".zst", ".manifest.json")
        {
            return Err(AuthorError::Invalid);
        }
        let spec = serde_json::to_string(receipt).map_err(|_| AuthorError::Invalid)?;
        if spec.len() > 1_048_576 {
            return Err(AuthorError::Invalid);
        }
        let fingerprint = Self::fingerprint(receipt);
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let owned: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM books b JOIN users u ON u.id=b.owner_user_id
             WHERE b.id=$1 AND b.owner_user_id=$2 AND u.account_state='active'
             AND NOT b.admin_hold AND NOT b.is_deleted
             AND b.visibility IN ('public','draft'))",
        )
        .bind(book)
        .bind(actor)
        .fetch_one(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if !owned {
            return Err(AuthorError::Conflict);
        }
        let mut seen = std::collections::HashSet::new();
        for f in &receipt.frames {
            if f.chapter_id <= 0
                || f.version_id <= 0
                || !(1..=200_000).contains(&f.size_bytes)
                || f.frame_bytes == 0
                || f.frame_offset
                    .checked_add(f.frame_bytes)
                    .is_none_or(|end| end > receipt.target.seek_table_start)
                || !seen.insert((f.chapter_id, f.version_id))
            {
                return Err(AuthorError::Invalid);
            }
            let exact: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                 SELECT 1 FROM chapters c
                 JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=$3
                 JOIN free_chapter_storage_objects s
                   ON s.chapter_id=c.id AND s.version_id=v.id
                 WHERE c.id=$1 AND c.book_id=$2
                   AND c.access_level='free' AND c.price_amount=0
                   AND NOT c.is_deleted AND NOT c.admin_hold
                   AND v.content_sha256=$4 AND s.sha256=$4 AND s.size_bytes=$5)",
            )
            .bind(f.chapter_id)
            .bind(book)
            .bind(f.version_id)
            .bind(f.content_sha256.as_slice())
            .bind(f.size_bytes as i64)
            .fetch_one(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
            if !exact {
                return Err(AuthorError::Conflict);
            }
        }
        let job: Option<i64> = sqlx::query_scalar(
            "INSERT INTO free_pack_jobs
             (actor_user_id,book_id,approved_fingerprint,expected_chapter_count)
             VALUES($1,$2,$3,$4)
             ON CONFLICT(book_id,approved_fingerprint) DO NOTHING RETURNING id",
        )
        .bind(actor)
        .bind(book)
        .bind(fingerprint.as_slice())
        .bind(receipt.frames.len() as i32)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let id = if let Some(id) = job {
            sqlx::query("INSERT INTO free_pack_job_specs(job_id,spec_text) VALUES($1,$2)")
                .bind(id)
                .bind(&spec)
                .execute(&mut *tx)
                .await
                .map_err(|_| AuthorError::Unavailable)?;
            id
        } else {
            let existing = sqlx::query(
                "SELECT j.id,j.actor_user_id,j.expected_chapter_count,j.status,s.spec_text
                 FROM free_pack_jobs j JOIN free_pack_job_specs s ON s.job_id=j.id
                 WHERE j.book_id=$1 AND j.approved_fingerprint=$2",
            )
            .bind(book)
            .bind(fingerprint.as_slice())
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| AuthorError::Unavailable)?
            .ok_or(AuthorError::Conflict)?;
            let existing_actor: i64 = existing
                .try_get("actor_user_id")
                .map_err(|_| AuthorError::Unavailable)?;
            let existing_count: i32 = existing
                .try_get("expected_chapter_count")
                .map_err(|_| AuthorError::Unavailable)?;
            let existing_status: String = existing
                .try_get("status")
                .map_err(|_| AuthorError::Unavailable)?;
            let existing_spec: String = existing
                .try_get("spec_text")
                .map_err(|_| AuthorError::Unavailable)?;
            if existing_actor != actor
                || existing_count as usize != receipt.frames.len()
                || existing_spec != spec
                || !matches!(
                    existing_status.as_str(),
                    "pending" | "running" | "succeeded"
                )
            {
                return Err(AuthorError::Conflict);
            }
            existing
                .try_get("id")
                .map_err(|_| AuthorError::Unavailable)?
        };
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(id)
    }

    /// SKIP LOCKED ensures two workers cannot claim the same generation.
    /// Reclaiming an expired lease *increments* the fencing epoch.
    pub async fn claim_pack_job(
        &self,
        lease_seconds: i32,
    ) -> Result<Option<PackJobClaim>, AuthorError> {
        if !(5..=900).contains(&lease_seconds) {
            return Err(AuthorError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let row = sqlx::query(
            "WITH next AS (
               SELECT j.id FROM free_pack_jobs j
               JOIN free_pack_job_specs s ON s.job_id=j.id
               WHERE j.status='pending'
                  OR (j.status='running' AND j.locked_until<=clock_timestamp())
               ORDER BY j.created_at,j.id
               FOR UPDATE OF j SKIP LOCKED LIMIT 1
             )
             UPDATE free_pack_jobs j SET status='running',
               lease_epoch=j.lease_epoch+1,
               locked_until=clock_timestamp()+($1::int * interval '1 second'),
               updated_at=clock_timestamp()
             FROM next WHERE j.id=next.id
             RETURNING j.id,j.lease_epoch,j.actor_user_id,j.book_id,
                       j.approved_fingerprint,j.expected_chapter_count",
        )
        .bind(lease_seconds)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        let Some(row) = row else {
            tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
            return Ok(None);
        };
        let id: i64 = row.try_get("id").map_err(|_| AuthorError::Unavailable)?;
        let epoch: i64 = row
            .try_get("lease_epoch")
            .map_err(|_| AuthorError::Unavailable)?;
        let actor: i64 = row
            .try_get("actor_user_id")
            .map_err(|_| AuthorError::Unavailable)?;
        let book: i64 = row
            .try_get("book_id")
            .map_err(|_| AuthorError::Unavailable)?;
        let fingerprint: Vec<u8> = row
            .try_get("approved_fingerprint")
            .map_err(|_| AuthorError::Unavailable)?;
        let count: i32 = row
            .try_get("expected_chapter_count")
            .map_err(|_| AuthorError::Unavailable)?;
        let spec: String =
            sqlx::query_scalar("SELECT spec_text FROM free_pack_job_specs WHERE job_id=$1")
                .bind(id)
                .fetch_one(&mut *tx)
                .await
                .map_err(|_| AuthorError::Unavailable)?;
        let receipt: PackPlacementReceipt =
            serde_json::from_str(&spec).map_err(|_| AuthorError::Conflict)?;
        if Self::fingerprint(&receipt).as_slice() != fingerprint.as_slice()
            || receipt.frames.len() != count as usize
        {
            return Err(AuthorError::Conflict);
        }
        tx.commit().await.map_err(|_| AuthorError::Unavailable)?;
        Ok(Some(PackJobClaim {
            job_id: id,
            epoch,
            actor_user_id: actor,
            book_id: book,
            receipt,
        }))
    }

    /// Renewal cannot resurrect an already expired, cancelled or stolen job.
    pub async fn renew_pack_job(
        &self,
        job: i64,
        epoch: i64,
        lease_seconds: i32,
    ) -> Result<bool, AuthorError> {
        if job <= 0 || epoch <= 0 || !(5..=900).contains(&lease_seconds) {
            return Err(AuthorError::Invalid);
        }
        let changed = sqlx::query(
            "UPDATE free_pack_jobs
             SET locked_until=clock_timestamp()+($3::int * interval '1 second'),
                 updated_at=clock_timestamp()
             WHERE id=$1 AND lease_epoch=$2 AND status='running'
               AND locked_until>clock_timestamp()",
        )
        .bind(job)
        .bind(epoch)
        .bind(lease_seconds)
        .execute(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        Ok(changed.rows_affected() == 1)
    }
}
