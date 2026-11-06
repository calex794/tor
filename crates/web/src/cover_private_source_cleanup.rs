//! MOD-COVER-01: opt-in cleanup of unused PRIVATE original uploads.
//! A source may be shared by multiple recipes, so B must retire the entire
//! job graph before this worker is allowed to delete its exact S3 object.
use std::{sync::Arc, time::Duration};
use tor_books::repositories::cover_source_cleanup::PgPrivateCoverSourceRemovals;
use tor_ingest::{domain::cover::CoverError, services::cover_jobs::CoverObjectStore};

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub struct PrivateSourceCleanupResult {
    pub claimed: usize,
    pub removed: usize,
    pub deferred: usize,
}

pub struct PrivateCoverSourceCleanup {
    ledger: PgPrivateCoverSourceRemovals,
    storage: Arc<dyn CoverObjectStore>,
}

impl PrivateCoverSourceCleanup {
    pub fn new(ledger: PgPrivateCoverSourceRemovals, storage: Arc<dyn CoverObjectStore>) -> Self {
        Self { ledger, storage }
    }

    /// No startup schedule and no HTTP endpoint. Call only by a deliberately
    /// configured operator after provisioning the source-retirement migration.
    pub async fn run_batch(&self) -> Result<PrivateSourceCleanupResult, CoverError> {
        self.ledger
            .stage_eligible(8)
            .await
            .map_err(|_| CoverError::Unavailable)?;
        let claims = self
            .ledger
            .claim(2)
            .await
            .map_err(|_| CoverError::Unavailable)?;
        let mut result = PrivateSourceCleanupResult {
            claimed: claims.len(),
            ..PrivateSourceCleanupResult::default()
        };
        // Two 12s S3 operations fit well inside the 60s fenced DB lease.
        for item in claims {
            let operation = tokio::time::timeout(
                Duration::from_secs(12),
                self.storage
                    .remove_private_staging_source(&item.storage_key),
            )
            .await;
            if matches!(operation, Ok(Ok(()))) {
                if !self
                    .ledger
                    .removed(item.id, item.lease_epoch)
                    .await
                    .map_err(|_| CoverError::Unavailable)?
                {
                    return Err(CoverError::LostLease);
                }
                result.removed += 1;
            } else {
                if !self
                    .ledger
                    .retry(item.id, item.lease_epoch)
                    .await
                    .map_err(|_| CoverError::Unavailable)?
                {
                    return Err(CoverError::LostLease);
                }
                result.deferred += 1;
            }
        }
        Ok(result)
    }
}
