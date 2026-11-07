//! Default no-network CDN mock for developer runs without test-cdn-loopback.
use std::sync::atomic::{AtomicUsize, Ordering};
use tor_web::cover_cdn_purge::{CdnPurgeError, CoverCdnApi, PurgeStatus};

#[derive(Default)]
pub(super) struct FakeCoverCdn {
    submits: AtomicUsize,
    polls: AtomicUsize,
}

impl FakeCoverCdn {
    pub(super) fn submissions(&self) -> usize {
        self.submits.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl CoverCdnApi for FakeCoverCdn {
    async fn submit(&self, path: &str) -> Result<String, CdnPurgeError> {
        assert!(path.starts_with("/public/covers/950/"));
        assert!(path.ends_with(".webp*"));
        let sequence = self.submits.fetch_add(1, Ordering::SeqCst);
        Ok(format!("ci-operation-{sequence}"))
    }

    async fn status(&self, _operation_id: &str) -> Result<PurgeStatus, CdnPurgeError> {
        let call = self.polls.fetch_add(1, Ordering::SeqCst);
        if call < 2 {
            Ok(PurgeStatus::Pending)
        } else if call == 2 {
            Ok(PurgeStatus::Failed)
        } else {
            Ok(PurgeStatus::Success)
        }
    }
}
