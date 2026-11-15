//! Bounded opt-in cleanup for private package covers only.
//! Not enabled during startup; invoke explicitly from trusted operations.
use std::{sync::Arc, time::Duration};
use tor_commerce::repositories::layer_cover_cleanup::PgPackageCoverVariantRemovals;
use tor_ingest::{domain::cover::CoverError, services::cover_jobs::CoverObjectStore};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PackageCoverCleanupResult {
    pub claimed: usize,
    pub removed: usize,
    pub deferred: usize,
}

pub struct PackageCoverVariantCleanup {
    ledger: PgPackageCoverVariantRemovals,
    storage: Arc<dyn CoverObjectStore>,
}

impl PackageCoverVariantCleanup {
    pub fn new(ledger: PgPackageCoverVariantRemovals, storage: Arc<dyn CoverObjectStore>) -> Self {
        Self { ledger, storage }
    }

    pub async fn run_batch(&self) -> Result<PackageCoverCleanupResult, CoverError> {
        self.ledger
            .stage_eligible(12)
            .await
            .map_err(|_| CoverError::Unavailable)?;
        let claims = self
            .ledger
            .claim(3)
            .await
            .map_err(|_| CoverError::Unavailable)?;
        let mut result = PackageCoverCleanupResult {
            claimed: claims.len(),
            ..PackageCoverCleanupResult::default()
        };
        for item in claims {
            // Never accept arbitrary storage keys: only the exact PG-backed
            // retired cover variant, checked again on claim, is used.
            let action = tokio::time::timeout(
                Duration::from_secs(12),
                self.storage
                    .remove_private_staging_variant(&item.storage_key),
            )
            .await;
            if matches!(action, Ok(Ok(()))) {
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
