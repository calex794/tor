//! S025: operator-only public CDN publication, origin withdrawal and purge.
//! No worker runs unless the operator explicitly opts in with PostgreSQL,
//! authenticated Book moderation, a separate public S3 bucket and Yandex API.

use crate::{
    cover_cdn_cleanup::CoverOriginCleanup,
    cover_cdn_purge::CoverCdnPurgeWorker,
    cover_delivery::CoverDelivery,
    yandex_cdn_purge::YandexCdnPurge,
};
use std::{sync::Arc, time::Duration};
use tokio::{task::JoinHandle, time::MissedTickBehavior};
use tor_books::repositories::{
    cover_publications::PgPublicCoverPublications,
    cover_slots::PgCoverSlots,
    PgCatalog,
};
use tor_ingest::{domain::cover::CoverError, services::cover_jobs::CoverObjectStore};

/// The S3/HTML feature flags alone must never start background cloud writes.
/// Requiring active cookie authentication also prevents accidental operation
/// in a public reader-only database that cannot moderate cover candidates.
pub fn worker_opt_in(
    setting: Option<&str>,
    demo: bool,
    postgres: bool,
    auth_enabled: bool,
    private_storage: bool,
    public_cdn_configured: bool,
) -> Result<bool, &'static str> {
    match setting {
        None | Some("0") => Ok(false),
        Some("1")
            if !demo
                && postgres
                && auth_enabled
                && private_storage
                && public_cdn_configured =>
        {
            Ok(true)
        }
        Some("1") => Err("S025 CDN lifecycle needs authenticated PG, private S3, public CDN"),
        Some(_) => Err("TOR_CDN_COVER_LIFECYCLE_WORKER_ENABLED must be 0 or 1"),
    }
}

struct CoverCdnLifecycle {
    registry: PgPublicCoverPublications,
    publisher: CoverDelivery,
    origin_cleanup: CoverOriginCleanup,
    cdn_purge: CoverCdnPurgeWorker,
}

impl CoverCdnLifecycle {
    async fn run_cycle(&self) {
        // Revoke previously withdrawn URLs before publishing additional covers.
        // Every stage owns a durable PG lease; transient failure cannot mean
        // success. Never log bearer, object keys, path or provider response.
        match self.cdn_purge.poll_batch().await {
            Ok(batch) if batch.claimed > 0 => {
                tracing::info!(completed = batch.completed, pending = batch.deferred, "S025 CDN status");
            }
            Ok(_) => {}
            Err(_) => tracing::warn!("S025 CDN status unavailable; durable retry pending"),
        }
        match self.origin_cleanup.run_batch().await {
            Ok(batch) if batch.claimed > 0 => {
                tracing::info!(deleted = batch.origin_deleted, retry = batch.retry_scheduled, "S025 origin retirement");
            }
            Ok(_) => {}
            Err(_) => tracing::warn!("S025 origin retirement unavailable; durable retry pending"),
        }
        match self.cdn_purge.submit_batch().await {
            Ok(batch) if batch.claimed > 0 => {
                tracing::info!(submitted = batch.submitted, retry = batch.retry_scheduled, "S025 CDN submission");
            }
            Ok(_) => {}
            Err(_) => tracing::warn!("S025 CDN submission unavailable; durable retry pending"),
        }
        let candidates = match self.registry.publication_candidates(3).await {
            Ok(candidates) => candidates,
            Err(_) => {
                tracing::warn!("S025 approved-publication scan unavailable");
                return;
            }
        };
        for (book_id, kind) in candidates {
            let Ok(id) = u64::try_from(book_id) else {
                tracing::warn!("S025 invalid approved-publication candidate");
                continue;
            };
            if self.publisher.publish_cdn_variant(id, kind).await.is_err() {
                // The reserved B identity remains durable after partial COPY.
                // A retry is eligible after the B-owned cooldown; never serve
                // the unconfirmed URL and do not mutate unrelated objects.
                tracing::warn!("S025 approved-cover publication retry pending");
            }
        }
    }
}

/// Caller owns JoinHandle, and must only call this after normal HTTP/PG
/// initialization. No new database pool or schema migration is created.
/// A static IAM token may expire: operator must rotate/monitor credentials.
pub fn start(
    catalog: PgCatalog,
    storage: Arc<dyn CoverObjectStore>,
    resource_id: String,
    bearer: String,
) -> Result<JoinHandle<()>, CoverError> {
    if storage.public_cdn_origin().is_none() {
        return Err(CoverError::Forbidden);
    }
    let client = Arc::new(YandexCdnPurge::new(resource_id, bearer)?);
    let registry = PgPublicCoverPublications::new(catalog.pool());
    let worker = CoverCdnLifecycle {
        registry: registry.clone(),
        publisher: CoverDelivery::new(PgCoverSlots::new(catalog.pool()), storage.clone())
            .with_publication_registry(registry.clone()),
        origin_cleanup: CoverOriginCleanup::new(registry.clone(), storage),
        cdn_purge: CoverCdnPurgeWorker::new(registry, client),
    };
    Ok(tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(30));
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            worker.run_cycle().await;
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_by_default_and_requires_all_operator_controls() {
        let allowed = |setting, demo, postgres, auth, private, public| {
            worker_opt_in(setting, demo, postgres, auth, private, public)
        };
        assert_eq!(allowed(None, false, true, true, true, true), Ok(false));
        assert_eq!(allowed(Some("0"), false, true, true, true, true), Ok(false));
        assert_eq!(allowed(Some("1"), false, true, true, true, true), Ok(true));
        for value in ["", "yes", "true", "2", "01"] {
            assert!(allowed(Some(value), false, true, true, true, true).is_err());
        }
        for prereq in [
            (true, true, true, true, true),
            (false, false, true, true, true),
            (false, true, false, true, true),
            (false, true, true, false, true),
            (false, true, true, true, false),
        ] {
            assert!(allowed(Some("1"), prereq.0, prereq.1, prereq.2, prereq.3, prereq.4).is_err());
        }
    }
}
