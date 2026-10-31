//! G-07 job lifecycle; SQLx remains in repositories and B owns slot mutation.
use crate::domain::cover::{CoverClaim, CoverError, CoverVariant};
use async_trait::async_trait;
use std::time::Duration;

#[async_trait]
pub trait CoverJobs: Send + Sync {
    async fn claim_next(&self) -> Result<Option<CoverClaim>, CoverError>;
    async fn existing_variants(&self, job_id: i64) -> Result<Vec<CoverVariant>, CoverError>;
    async fn record_pair(
        &self,
        job_id: i64,
        epoch: i64,
        variants: &[CoverVariant; 2],
    ) -> Result<(), CoverError>;
    async fn conclude(&self, job_id: i64, epoch: i64, applied: bool) -> Result<(), CoverError>;
    async fn record_failure(
        &self,
        job_id: i64,
        epoch: i64,
        retryable: bool,
        code: &'static str,
    ) -> Result<(), CoverError>;
}

#[async_trait]
pub trait CoverObjectStore: Send + Sync {
    /// A source must be verified by a signed PUT, HEAD, and bounded GET.
    /// Stores without source-upload support fail closed.
    async fn put_source_verified(
        &self,
        _key: &str,
        _bytes: &[u8],
        _mime: &str,
        _sha256: &[u8; 32],
    ) -> Result<(), CoverError> {
        Err(CoverError::Forbidden)
    }
    /// Only a separately configured, operator-approved public S3 endpoint may
    /// issue direct-reader URLs. Legacy/mock providers remain proxy-only.
    fn direct_read_origin(&self) -> Option<&str> {
        None
    }

    /// Sign only a previously authorized public cover variant. No source uploads.
    /// This method must never read or return the object bytes through TOR.
    async fn presign_variant_get(
        &self,
        _key: &str,
        _ttl: Duration,
    ) -> Result<String, CoverError> {
        Err(CoverError::Forbidden)
    }

    /// Implementations MUST stop at max_bytes+1 while streaming, never collect unbounded.
    async fn get_bounded(&self, key: &str, max_bytes: usize) -> Result<Vec<u8>, CoverError>;
    /// MUST verify HEAD/sha/length after idempotent upload.
    async fn put_verified(&self, key: &str, bytes: &[u8]) -> Result<(), CoverError>;
    /// MUST HEAD/verify existing output on worker resume before B attachment.
    async fn exists_verified(
        &self,
        key: &str,
        sha256: &[u8; 32],
        size: i64,
    ) -> Result<bool, CoverError>;
}

#[async_trait]
pub trait BookCoverAttach: Send + Sync {
    /// B CAS checks current owner + expected revision and two verified variants.
    /// false means superseded; true includes idempotently already attached.
    async fn attach_ready(&self, job_id: i64, epoch: i64) -> Result<bool, CoverError>;
}
