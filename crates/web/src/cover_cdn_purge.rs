//! S025: leased CDN purge processing after confirmed public S3 origin removal.
//! This worker is not enabled by the live application.
use crate::yandex_cdn_purge::{PurgeState, YandexCdnPurge};
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};
use tor_books::repositories::cover_publications::PgPublicCoverPublications;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PurgeStatus {
    Pending,
    Success,
    Failed,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CdnPurgeError {
    Invalid,
    Unavailable,
    LostLease,
}

#[async_trait]
pub trait CoverCdnApi: Send + Sync {
    async fn submit(&self, exact_path: &str) -> Result<String, CdnPurgeError>;
    async fn status(&self, operation_id: &str) -> Result<PurgeStatus, CdnPurgeError>;
}

#[async_trait]
impl CoverCdnApi for YandexCdnPurge {
    async fn submit(&self, path: &str) -> Result<String, CdnPurgeError> {
        YandexCdnPurge::submit(self, path)
            .await
            .map_err(|_| CdnPurgeError::Unavailable)
    }

    async fn status(&self, id: &str) -> Result<PurgeStatus, CdnPurgeError> {
        match YandexCdnPurge::poll(self, id)
            .await
            .map_err(|_| CdnPurgeError::Unavailable)?
        {
            PurgeState::Running => Ok(PurgeStatus::Pending),
            PurgeState::Succeeded => Ok(PurgeStatus::Success),
            PurgeState::Failed => Ok(PurgeStatus::Failed),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub struct CdnPurgeBatch {
    pub claimed: usize,
    pub submitted: usize,
    pub completed: usize,
    pub deferred: usize,
    pub retry_scheduled: usize,
}

pub struct CoverCdnPurgeWorker {
    ledger: PgPublicCoverPublications,
    api: Arc<dyn CoverCdnApi>,
}

impl CoverCdnPurgeWorker {
    pub fn new(ledger: PgPublicCoverPublications, api: Arc<dyn CoverCdnApi>) -> Self {
        Self { ledger, api }
    }

    /// A bounded, explicitly invoked stage. Retrying an ambiguous submit is
    /// safe: purging the same immutable path twice never broadens its scope.
    pub async fn submit_batch(&self) -> Result<CdnPurgeBatch, CdnPurgeError> {
        let claims = self
            .ledger
            .claim_purge_submissions(3)
            .await
            .map_err(|_| CdnPurgeError::Unavailable)?;
        let mut result = CdnPurgeBatch {
            claimed: claims.len(),
            ..CdnPurgeBatch::default()
        };
        for item in claims {
            let attempt =
                tokio::time::timeout(Duration::from_secs(12), self.api.submit(&item.purge_path()))
                    .await;
            match attempt {
                Ok(Ok(operation_id)) => {
                    if !self
                        .ledger
                        .purge_submitted_fenced(item.id, item.lease_epoch, &operation_id)
                        .await
                        .map_err(|_| CdnPurgeError::Unavailable)?
                    {
                        return Err(CdnPurgeError::LostLease);
                    }
                    result.submitted += 1;
                }
                _ => {
                    if !self
                        .ledger
                        .retry_purge_submission(item.id, item.lease_epoch)
                        .await
                        .map_err(|_| CdnPurgeError::Unavailable)?
                    {
                        return Err(CdnPurgeError::LostLease);
                    }
                    result.retry_scheduled += 1;
                }
            }
        }
        Ok(result)
    }

    /// Only a confirmed successful Operation.Get can mark an object purged.
    /// Pending/unknown results keep the operation ID for later polling.
    pub async fn poll_batch(&self) -> Result<CdnPurgeBatch, CdnPurgeError> {
        let claims = self
            .ledger
            .claim_purge_polls(3)
            .await
            .map_err(|_| CdnPurgeError::Unavailable)?;
        let mut result = CdnPurgeBatch {
            claimed: claims.len(),
            ..CdnPurgeBatch::default()
        };
        for (item, operation_id) in claims {
            let response =
                tokio::time::timeout(Duration::from_secs(12), self.api.status(&operation_id)).await;
            let changed = match response {
                Ok(Ok(PurgeStatus::Success)) => {
                    let updated = self
                        .ledger
                        .purge_completed_fenced(item.id, item.lease_epoch, &operation_id)
                        .await;
                    result.completed += 1;
                    updated
                }
                Ok(Ok(PurgeStatus::Failed)) => {
                    let updated = self
                        .ledger
                        .purge_failed_fenced(item.id, item.lease_epoch, &operation_id)
                        .await;
                    result.retry_scheduled += 1;
                    updated
                }
                _ => {
                    let updated = self
                        .ledger
                        .defer_purge_poll(item.id, item.lease_epoch, &operation_id)
                        .await;
                    result.deferred += 1;
                    updated
                }
            }
            .map_err(|_| CdnPurgeError::Unavailable)?;
            if !changed {
                return Err(CdnPurgeError::LostLease);
            }
        }
        Ok(result)
    }
}
