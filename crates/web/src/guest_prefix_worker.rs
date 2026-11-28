//! AUTH-READ: internal one-claim source preparation bridge, not an HTTP API.
//! Consumer settlement is separate from source acquisition; no startup loop.
use std::time::Duration;
use tor_books::{
    repositories::guest_prefix::{
        GuestPrefixJob, GuestPrefixPreparation, GuestPrefixSource, PgGuestPrefix,
    },
    services::authoring::AuthorError,
};

mod consumer;
pub use consumer::{GuestPrefixConsumer, GuestPrefixWorkerOutcome};

#[cfg(feature = "s3-free")]
mod remote;
#[cfg(feature = "s3-free")]
pub use remote::prepare_verified_job;

/// PostgreSQL-only runtime: never fetch a bodyless source without the
/// configured private provider. The repository repeats the claim at commit.
pub async fn prepare_legacy_job(
    repo: &PgGuestPrefix,
    job: &GuestPrefixJob,
) -> Result<Option<GuestPrefixPreparation>, AuthorError> {
    tokio::time::timeout(Duration::from_secs(20), async {
        match repo.claimed_source(job).await? {
            None => Ok(None),
            Some(GuestPrefixSource::Legacy(source)) => {
                repo.prepare_claimed(job, &source).await.map(Some)
            }
            Some(_) => Err(AuthorError::Unavailable),
        }
    })
    .await
    .map_err(|_| AuthorError::Unavailable)?
}
