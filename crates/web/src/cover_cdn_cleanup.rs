//! S025: bounded recovery of withdrawn immutable public covers from S3.
//! This is only the origin-delete stage. Yandex CDN purge is a separate,
//! durable follow-up; origin_deleted MUST NOT be interpreted as purged.
use std::{sync::Arc, time::Duration};
use tor_books::repositories::cover_publications::PgPublicCoverPublications;
use tor_ingest::{domain::cover::CoverError, services::cover_jobs::CoverObjectStore};

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub struct CoverCleanupResult {
    pub claimed: usize,
    pub origin_deleted: usize,
    pub retry_scheduled: usize,
}

pub struct CoverOriginCleanup {
    ledger: PgPublicCoverPublications,
    storage: Arc<dyn CoverObjectStore>,
}

impl CoverOriginCleanup {
    pub fn new(ledger: PgPublicCoverPublications, storage: Arc<dyn CoverObjectStore>) -> Self {
        Self { ledger, storage }
    }

    /// Not called by production startup. The operator must configure a public
    /// origin and explicitly schedule this worker after provisioning CDN purge.
    /// Lease and retry state reside in PostgreSQL and survive process restarts.
    pub async fn run_batch(&self) -> Result<CoverCleanupResult, CoverError> {
        if self.storage.public_cdn_origin().is_none() {
            return Err(CoverError::Forbidden);
        }
        let claims = self
            .ledger
            // The lease lasts one minute; 3 x 15s plus DB overhead fits.
            // Never claim 10 jobs when a slow S3 call could expire the lease.
            .claim_removals(3)
            .await
            .map_err(|_| CoverError::Unavailable)?;
        let mut result = CoverCleanupResult {
            claimed: claims.len(),
            ..CoverCleanupResult::default()
        };
        for item in claims {
            let book_id = u64::try_from(item.book_id).map_err(|_| CoverError::Invalid)?;
            let operation = tokio::time::timeout(
                Duration::from_secs(15),
                self.storage.remove_public_cover(
                    book_id,
                    item.slot_revision,
                    &item.variant_name,
                    &item.sha256,
                ),
            )
            .await;
            if matches!(operation, Ok(Ok(_))) {
                if !self
                    .ledger
                    .origin_deleted(item.id, item.lease_epoch)
                    .await
                    .map_err(|_| CoverError::Unavailable)?
                {
                    // A second process may have reclaimed the lease; never
                    // mark its work as completed.
                    return Err(CoverError::LostLease);
                }
                result.origin_deleted += 1;
            } else {
                if !self
                    .ledger
                    .retry_removal(item.id, item.lease_epoch)
                    .await
                    .map_err(|_| CoverError::Unavailable)?
                {
                    return Err(CoverError::LostLease);
                }
                result.retry_scheduled += 1;
            }
        }
        Ok(result)
    }
}
