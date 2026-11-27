//! Bounded opt-in cleanup for private package covers only.
//! Not enabled during startup; invoke explicitly from trusted operations.
use std::sync::Arc;
use tor_commerce::repositories::layer_cover_cleanup::PgPackageCoverVariantRemovals;
use tor_ingest::{domain::cover::CoverError, services::cover_jobs::CoverObjectStore};

use crate::cover_private_cleanup::removal::{finish_private_removal, PRIVATE_REMOVAL_TIMEOUT};

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
            if finish_private_removal(
                PRIVATE_REMOVAL_TIMEOUT,
                self.storage
                    .remove_private_staging_variant(&item.storage_key),
                self.ledger.removed(item.id, item.lease_epoch),
                self.ledger.retry(item.id, item.lease_epoch),
            )
            .await?
            {
                result.removed += 1;
            } else {
                result.deferred += 1;
            }
        }
        Ok(result)
    }
}
