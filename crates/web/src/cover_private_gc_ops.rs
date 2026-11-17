//! #667: one bounded PRIVATE cover GC pass, variants before shared originals.
//!
//! This is a library composition for an eventual explicitly approved manual
//! operator entrypoint. Neither HTTP startup nor any scheduler invokes it.
//! It is NOT a public CDN purge worker and does not grant cleanup permission.
use std::sync::Arc;
use tor_books::repositories::{
    cover_private_cleanup::PgPrivateCoverVariantRemovals,
    cover_source_cleanup::PgPrivateCoverSourceRemovals, PgCatalog,
};
use tor_ingest::{domain::cover::CoverError, services::cover_jobs::CoverObjectStore};

use crate::{
    cover_private_cleanup::{PrivateCoverCleanupResult, PrivateCoverVariantCleanup},
    cover_private_source_cleanup::{PrivateCoverSourceCleanup, PrivateSourceCleanupResult},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PrivateCoverGcCycle {
    pub variants: PrivateCoverCleanupResult,
    /// None means variant removal was deferred and the source pass was skipped.
    pub sources: Option<PrivateSourceCleanupResult>,
}

pub struct PrivateCoverGcCoordinator {
    variants: PrivateCoverVariantCleanup,
    sources: PrivateCoverSourceCleanup,
}

fn can_retire_sources(variants: &PrivateCoverCleanupResult) -> bool {
    // A failed or uncertain variant deletion is not a reason to attempt
    // retiring its shared original in the same operator pass. B additionally
    // rechecks the full source/variant graph under its own lease.
    variants.deferred == 0
}

impl PrivateCoverGcCoordinator {
    pub fn new(catalog: &PgCatalog, storage: Arc<dyn CoverObjectStore>) -> Self {
        let pool = catalog.pool();
        Self {
            variants: PrivateCoverVariantCleanup::new(
                PgPrivateCoverVariantRemovals::new(pool.clone()),
                storage.clone(),
            ),
            sources: PrivateCoverSourceCleanup::new(
                PgPrivateCoverSourceRemovals::new(pool),
                storage,
            ),
        }
    }

    /// Exactly one bounded pass, no timer, loop, startup hook or HTTP endpoint.
    /// An authorized caller must supply an isolated credential and a separate
    /// approved retention policy before using this with a real S3 bucket.
    pub async fn run_single_bounded_batch(&self) -> Result<PrivateCoverGcCycle, CoverError> {
        let variants = self.variants.run_batch().await?;
        let sources = if can_retire_sources(&variants) {
            Some(self.sources.run_batch().await?)
        } else {
            None
        };
        Ok(PrivateCoverGcCycle { variants, sources })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_phase_is_denied_after_variant_retries() {
        assert!(can_retire_sources(&PrivateCoverCleanupResult {
            claimed: 3,
            removed: 3,
            deferred: 0,
        }));
        assert!(can_retire_sources(&PrivateCoverCleanupResult::default()));
        assert!(!can_retire_sources(&PrivateCoverCleanupResult {
            claimed: 3,
            removed: 2,
            deferred: 1,
        }));
        assert!(!can_retire_sources(&PrivateCoverCleanupResult {
            claimed: 1,
            removed: 0,
            deferred: 1,
        }));
    }
}
