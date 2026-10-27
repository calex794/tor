//! G-09/S3-12: one bounded orphan cleanup unit, not an HTTP route.
//! The PG claim is fenced before deleting S3 bytes; B can only attach ready
//! rows and may never attach a row claimed as deleting. Replays are safe.
use async_trait::async_trait;
use super::super::{
    domain::{gc::GcClaim, provider::StorageError},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GcError {
    Unavailable,
    LostLease,
    Invalid,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GcOutcome {
    Idle,
    Deleted,
    Retrying,
    NeedsAttention,
}

#[async_trait]
pub trait OrphanGcRegistry: Send + Sync {
    async fn claim_due(&self) -> Result<Option<GcClaim>, GcError>;
    async fn finish(&self, claim: &GcClaim) -> Result<(), GcError>;
    async fn retry(&self, claim: &GcClaim, code: &'static str) -> Result<(), GcError>;
}

#[async_trait]
pub trait OrphanDeletePort: Send + Sync {
    /// Only a verified, fenced database claim may authorize object deletion.
    async fn delete_claimed(&self, claim: &GcClaim) -> Result<(), StorageError>;
}

/// The worker does at most one delete per invocation, so a caller can bound
/// CPU, S3 requests, memory and interruptions. No private bucket/object URL
/// or failure debug message is logged.
pub async fn run_one<R, S>(registry: &R, storage: &S) -> Result<GcOutcome, GcError>
where
    R: OrphanGcRegistry,
    S: OrphanDeletePort,
{
    let Some(claim) = registry.claim_due().await? else {
        return Ok(GcOutcome::Idle);
    };
    match storage.delete_claimed(&claim).await {
        Ok(()) => {
            registry.finish(&claim).await?;
            Ok(GcOutcome::Deleted)
        }
        Err(error) => {
            let code = match error {
                StorageError::Unavailable => "unavailable",
                StorageError::Corrupt => "corrupt",
                StorageError::Conflict => "conflict",
                StorageError::Invalid => "invalid",
            };
            registry.retry(&claim, code).await?;
            Ok(if claim.attempts() >= 8 {
                GcOutcome::NeedsAttention
            } else {
                GcOutcome::Retrying
            })
        }
    }
}
