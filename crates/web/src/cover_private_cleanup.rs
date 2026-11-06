//! MOD-COVER-01: opt-in cleanup of retired PRIVATE staged WebP variants.
 //! It is separate from public origin removal and CDN invalidation.
use std::{sync::Arc, time::Duration};
use tor_books::repositories::cover_private_cleanup::PgPrivateCoverVariantRemovals;
use tor_ingest::{domain::cover::CoverError, services::cover_jobs::CoverObjectStore};

#[derive(Debug, Default, Copy, Clone, Eq, PartialEq)]
pub struct PrivateCoverCleanupResult {
    pub claimed: usize,
    pub removed: usize,
    pub deferred: usize,
}

pub struct PrivateCoverVariantCleanup {
    ledger: PgPrivateCoverVariantRemovals,
    storage: Arc<dyn CoverObjectStore>,
}

impl PrivateCoverVariantCleanup {
    pub fn new(ledger: PgPrivateCoverVariantRemovals, storage: Arc<dyn CoverObjectStore>) -> Self {
        Self { ledger, storage }
    }

    /// Never enabled by startup. Only a specifically configured operator or
    /// isolated CI scenario may invoke this bounded service.
    pub async fn run_batch(&self) -> Result<PrivateCoverCleanupResult, CoverError> {
        self.ledger
            .stage_eligible(12)
            .await
            .map_err(|_| CoverError::Unavailable)?;
        // At most 3 * 12 seconds fits comfortably inside a one-minute lease.
        let claims = self
            .ledger
            .claim(3)
            .await
            .map_err(|_| CoverError::Unavailable)?;
        let mut result = PrivateCoverCleanupResult {
            claimed: claims.len(),
            ..PrivateCoverCleanupResult::default()
        };
        for item in claims {
            let operation = tokio::time::timeout(
                Duration::from_secs(12),
                self.storage.remove_private_staging_variant(&item.storage_key),
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
