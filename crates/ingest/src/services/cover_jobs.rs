//! G-07 job lifecycle; SQLx remains in repositories and B owns slot mutation.
use crate::domain::cover::{CoverClaim, CoverError, CoverVariant};
use async_trait::async_trait;

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
    /// Direct public cover delivery is opt-in. Unconfigured/fake stores refuse
    /// grants and never expose a private S3 bucket or object key.
    fn direct_read_origin(&self) -> Option<&str> {
        None
    }

    /// Sign only a validated, attached public variant after B has checked ACL.
    /// This method is a storage capability, NOT authorization by itself.
    async fn presign_variant_get(
        &self,
        _key: &str,
        _ttl: std::time::Duration,
    ) -> Result<String, CoverError> {
        Err(CoverError::Forbidden)
    }
    /// Opt-in immutable published WebP storage, separate from the private source.
    /// The caller must first verify the current book/public-cover ACL in B.
    fn public_cdn_origin(&self) -> Option<&str> {
        None
    }

    /// Copy one verified public version to a distinct CDN origin bucket.
    /// No private object key or S3 credentials are returned to the browser.
    async fn publish_public_cover(
        &self,
        _book_id: u64,
        _revision: i64,
        _variant: &str,
        _source_key: &str,
        _sha256: &[u8; 32],
        _size_bytes: i64,
    ) -> Result<String, CoverError> {
        Err(CoverError::Forbidden)
    }

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
