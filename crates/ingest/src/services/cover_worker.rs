//! G worker: bounded source verification, CPU work, verified variant staging,
//! then B's atomic slot command. No HTTP callback or in-memory idempotency cache.
use crate::domain::cover::{CoverClaim, CoverError, CoverVariant};
use crate::services::{
    cover_jobs::{BookCoverAttach, CoverJobs, CoverObjectStore},
    image_pipeline::{process_cover, MAX_SOURCE_BYTES, RECIPE_VERSION},
};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerResult {
    Idle,
    Attached,
    Superseded,
    RetryOrQuarantine,
}

pub struct CoverWorker<'a, J: CoverJobs, S: CoverObjectStore, B: BookCoverAttach> {
    pub jobs: &'a J,
    pub storage: &'a S,
    pub books: &'a B,
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn hex_digest(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|value| format!("{value:02x}")).collect()
}

impl<J: CoverJobs, S: CoverObjectStore, B: BookCoverAttach> CoverWorker<'_, J, S, B> {
    pub async fn run_one(&self) -> Result<WorkerResult, CoverError> {
        let Some(claim) = self.jobs.claim_next().await? else {
            return Ok(WorkerResult::Idle);
        };
        let done = self.process_claim(&claim).await;
        match done {
            Ok(applied) => {
                self.jobs
                    .conclude(claim.job_id, claim.epoch, applied)
                    .await?;
                Ok(if applied {
                    WorkerResult::Attached
                } else {
                    WorkerResult::Superseded
                })
            }
            Err(CoverError::LostLease) => Err(CoverError::LostLease),
            Err(problem) => {
                let retryable = problem == CoverError::Unavailable;
                let code = match problem {
                    CoverError::Invalid => "invalid_source",
                    CoverError::Forbidden => "access_denied",
                    CoverError::Conflict => "variant_conflict",
                    CoverError::LostLease => unreachable!(),
                    CoverError::Unavailable => "store_or_pg_unavailable",
                };
                self.jobs
                    .record_failure(claim.job_id, claim.epoch, retryable, code)
                    .await?;
                Ok(WorkerResult::RetryOrQuarantine)
            }
        }
    }

    async fn process_claim(&self, claim: &CoverClaim) -> Result<bool, CoverError> {
        if claim.recipe_version != RECIPE_VERSION
            || !(1..=MAX_SOURCE_BYTES as i64).contains(&claim.source_size)
        {
            return Err(CoverError::Invalid);
        }
        let existing = self.jobs.existing_variants(claim.job_id).await?;
        if existing.is_empty() {
            let source = self
                .storage
                .get_bounded(&claim.source_key, MAX_SOURCE_BYTES)
                .await?;
            if source.len() as i64 != claim.source_size || sha256(&source) != claim.source_sha256 {
                return Err(CoverError::Invalid);
            }
            let mime = claim.source_mime.clone();
            // Decode/crop/WebP is CPU-intensive; never run it on Tokio's executor.
            let images = tokio::task::spawn_blocking(move || process_cover(&source, &mime))
                .await
                .map_err(|_| CoverError::Unavailable)?
                .map_err(|_| CoverError::Invalid)?;
            if images.len() != 2 {
                return Err(CoverError::Invalid);
            }
            let mut pair = Vec::with_capacity(2);
            for variant in images {
                let hash = sha256(&variant.bytes);
                let key = format!(
                    "staging/covers/variants/{}/{}/{}-{}.webp",
                    claim.job_id,
                    claim.epoch,
                    variant.name,
                    hex_digest(&hash)
                );
                self.storage.put_verified(&key, &variant.bytes).await?;
                if !self
                    .storage
                    .exists_verified(&key, &hash, variant.bytes.len() as i64)
                    .await?
                {
                    return Err(CoverError::Unavailable);
                }
                pair.push(CoverVariant {
                    name: variant.name.to_owned(),
                    storage_key: key,
                    sha256: hash,
                    size_bytes: variant.bytes.len() as i64,
                });
            }
            let pair: [CoverVariant; 2] = pair.try_into().map_err(|_| CoverError::Invalid)?;
            self.jobs
                .record_pair(claim.job_id, claim.epoch, &pair)
                .await?;
        } else {
            // A previous worker may have staged both artifacts and crashed
            // before B's slot transaction. Verify S3 again, never trust PG alone.
            if existing.len() != 2 || existing[0].name != "card" || existing[1].name != "main" {
                return Err(CoverError::Conflict);
            }
            for variant in &existing {
                if !self
                    .storage
                    .exists_verified(&variant.storage_key, &variant.sha256, variant.size_bytes)
                    .await?
                {
                    return Err(CoverError::Unavailable);
                }
            }
        }
        self.books.attach_ready(claim.job_id, claim.epoch).await
    }
}
