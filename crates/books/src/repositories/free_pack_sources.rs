//! DROP-R4b3: B-authoritative source inventory for a claimed PACK job.
//! No S3 access, no rights change, and no PostgreSQL transaction across I/O.
use super::free_pack_attach::PgFreePackCatalog;
use super::free_pack_jobs::PackJobClaim;
use crate::services::authoring::AuthorError;
use sqlx::Row;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ApprovedPackSource {
    pub chapter_id: i64,
    pub version_id: i64,
    pub content_sha256: [u8; 32],
    pub size_bytes: u64,
    pub bucket: String,
}

impl PgFreePackCatalog {
    /// B is the only authority for original SINGLE location and content hashes.
    /// The claim's embedded descriptor cannot name a different S3 source bucket.
    /// Concurrency with moderation or lease theft is safe: finish_pack_job
    /// checks every source and the current epoch again under PG row locks.
    pub async fn approved_pack_sources(
        &self,
        claim: &PackJobClaim,
    ) -> Result<Vec<ApprovedPackSource>, AuthorError> {
        if claim.job_id <= 0
            || claim.epoch <= 0
            || claim.actor_user_id <= 0
            || claim.book_id <= 0
            || claim.receipt.frames.is_empty()
            || claim.receipt.frames.len() > 4096
        {
            return Err(AuthorError::Invalid);
        }
        // A forged 4,096-member spec must never make G allocate hundreds
        // of MiB by fetching all SINGLEs before the Zstd builder rejects it.
        const MAX_DECODED_PACK_SOURCE_BYTES: u64 = 128 * 1024 * 1024;
        let budget = claim.receipt.frames.iter().try_fold(0_u64, |total, f| {
            total.checked_add(f.size_bytes)
        });
        if budget.is_none_or(|total| total > MAX_DECODED_PACK_SOURCE_BYTES) {
            return Err(AuthorError::Invalid);
        }
        let fingerprint = PgFreePackCatalog::fingerprint(&claim.receipt);
        let valid: bool = sqlx::query_scalar(
            "SELECT EXISTS (
               SELECT 1 FROM free_pack_jobs j
               JOIN books b ON b.id=j.book_id AND b.owner_user_id=j.actor_user_id
               JOIN users u ON u.id=b.owner_user_id
               WHERE j.id=$1 AND j.lease_epoch=$2 AND j.status='running'
                 AND j.actor_user_id=$3 AND j.book_id=$4
                 AND j.approved_fingerprint=$5 AND
                     j.expected_chapter_count=$6
                 AND j.locked_until>clock_timestamp()
                 AND u.account_state='active' AND NOT b.admin_hold AND NOT b.is_deleted
                 AND b.visibility IN ('draft','public')
             )",
        )
        .bind(claim.job_id)
        .bind(claim.epoch)
        .bind(claim.actor_user_id)
        .bind(claim.book_id)
        .bind(fingerprint.as_slice())
        .bind(claim.receipt.frames.len() as i32)
        .fetch_one(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        if !valid {
            return Err(AuthorError::Conflict);
        }
        let mut sources = Vec::with_capacity(claim.receipt.frames.len());
        for frame in &claim.receipt.frames {
            let row = sqlx::query(
                "SELECT s.bucket, s.object_key FROM chapters c
                 JOIN chapter_versions v ON v.chapter_id=c.id AND v.id=$3
                 JOIN free_chapter_storage_objects s
                     ON s.chapter_id=c.id AND s.version_id=v.id
                 WHERE c.id=$1 AND c.book_id=$2 AND c.access_level='free'
                   AND c.price_amount=0 AND NOT c.is_deleted AND NOT c.admin_hold
                   AND v.content_sha256=$4 AND s.sha256=$4 AND s.size_bytes=$5",
            )
            .bind(frame.chapter_id)
            .bind(claim.book_id)
            .bind(frame.version_id)
            .bind(frame.content_sha256.as_slice())
            .bind(frame.size_bytes as i64)
            .fetch_optional(&self.pool)
            .await
            .map_err(|_| AuthorError::Unavailable)?
            .ok_or(AuthorError::Conflict)?;
            let bucket: String = row
                .try_get("bucket")
                .map_err(|_| AuthorError::Unavailable)?;
            let key: String = row
                .try_get("object_key")
                .map_err(|_| AuthorError::Unavailable)?;
            if bucket != claim.receipt.target.bucket
                || key
                    != format!(
                        "private/free-chapters/{}/{}.md",
                        frame.chapter_id, frame.version_id
                    )
            {
                return Err(AuthorError::Conflict);
            }
            sources.push(ApprovedPackSource {
                chapter_id: frame.chapter_id,
                version_id: frame.version_id,
                content_sha256: frame.content_sha256,
                size_bytes: frame.size_bytes,
                bucket,
            });
        }
        Ok(sources)
    }

    /// Release only the current live lease; old workers cannot requeue or
    /// fail a newer epoch. Retry has a fixed cool-down and bounded attempts.
    pub async fn settle_pack_job(
        &self,
        claim: &PackJobClaim,
        retry: bool,
    ) -> Result<bool, AuthorError> {
        if claim.job_id <= 0 || claim.epoch <= 0 {
            return Err(AuthorError::Invalid);
        }
        let retry = retry && claim.epoch < 8;
        let status = if retry { "pending" } else { "failed" };
        let changed = sqlx::query(
            "UPDATE free_pack_jobs SET status=$3,
               locked_until=CASE WHEN $3='pending'
                 THEN clock_timestamp()+interval '30 seconds' ELSE NULL END,
               updated_at=clock_timestamp(),
               finished_at=CASE WHEN $3='failed' THEN clock_timestamp()
                                ELSE NULL END
             WHERE id=$1 AND lease_epoch=$2 AND status='running'
               AND locked_until>clock_timestamp()",
        )
        .bind(claim.job_id)
        .bind(claim.epoch)
        .bind(status)
        .execute(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        Ok(changed.rows_affected() == 1)
    }

    /// A missing R4a/R4b1 table is a hard startup failure for the opt-in runner.
    pub async fn probe_pack_worker_schema(&self) -> Result<(), AuthorError> {
        sqlx::query(
            "SELECT j.id,s.job_id FROM free_pack_jobs j
             JOIN free_pack_job_specs s ON s.job_id=j.id WHERE false",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
        Ok(())
    }
}
