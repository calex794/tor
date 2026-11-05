//! S025: leased, restartable selective CDN invalidation (default-off).
//! Never consider a submitted HTTP request proof of cache invalidation.
use async_trait::async_trait;
use std::sync::Arc;
use tor_books::repositories::cover_purge_queue::{PgCdnPurgeQueue, PurgeStage};
use tor_ingest::domain::cover::CoverError;

use crate::yandex_cdn_purge::{PurgeState, YandexCdnPurge};

#[async_trait]
pub trait CdnPurgeProvider: Send + Sync {
    async fn submit(&self, path: &str) -> Result<String, CoverError>;
    async fn poll(&self, id: &str) -> Result<PurgeState, CoverError>;
}

#[async_trait]
impl CdnPurgeProvider for YandexCdnPurge {
    async fn submit(&self, path: &str) -> Result<String, CoverError> {
        YandexCdnPurge::submit(self, path).await
    }

    async fn poll(&self, id: &str) -> Result<PurgeState, CoverError> {
        YandexCdnPurge::poll(self, id).await
    }
}

#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub struct CdnPurgeProgress {
    pub claimed: usize,
    pub submitted: usize,
    pub completed: usize,
    pub rescheduled: usize,
}

pub struct CdnPurgeWorker {
    queue: PgCdnPurgeQueue,
    provider: Arc<dyn CdnPurgeProvider>,
}

impl CdnPurgeWorker {
    pub fn new(queue: PgCdnPurgeQueue, provider: Arc<dyn CdnPurgeProvider>) -> Self {
        Self { queue, provider }
    }

    /// A bounded one-shot batch. The operator must explicitly start an
    /// isolated runner; TOR's ordinary server startup never calls this.
    pub async fn run_batch(&self) -> Result<CdnPurgeProgress, CoverError> {
        let claims = self
            .queue
            .claim_batch(2)
            .await
            .map_err(|_| CoverError::Unavailable)?;
        let mut progress = CdnPurgeProgress {
            claimed: claims.len(),
            ..CdnPurgeProgress::default()
        };
        for claim in claims {
            let saved = match claim.stage {
                PurgeStage::Submit => match self.provider.submit(&claim.purge_path()).await {
                    Ok(operation) => {
                        let changed = self
                            .queue
                            .mark_submitted(claim.id, claim.epoch, &operation)
                            .await
                            .map_err(|_| CoverError::Unavailable)?;
                        if changed {
                            progress.submitted += 1;
                        }
                        changed
                    }
                    Err(_) => {
                        let changed = self
                            .queue
                            .reschedule(claim.id, claim.epoch, true)
                            .await
                            .map_err(|_| CoverError::Unavailable)?;
                        if changed {
                            progress.rescheduled += 1;
                        }
                        changed
                    }
                },
                PurgeStage::Poll => {
                    let operation = claim.operation_id.as_deref().ok_or(CoverError::Invalid)?;
                    let state = self.provider.poll(operation).await;
                    let changed = match state {
                        Ok(PurgeState::Succeeded) => {
                            let saved = self
                                .queue
                                .mark_completed(claim.id, claim.epoch, operation)
                                .await
                                .map_err(|_| CoverError::Unavailable)?;
                            if saved {
                                progress.completed += 1;
                            }
                            saved
                        }
                        Ok(PurgeState::Failed) => {
                            let saved = self
                                .queue
                                .retry_failed_operation(claim.id, claim.epoch, operation)
                                .await
                                .map_err(|_| CoverError::Unavailable)?;
                            if saved {
                                progress.rescheduled += 1;
                            }
                            saved
                        }
                        Ok(PurgeState::Running) | Err(_) => {
                            let saved = self
                                .queue
                                .reschedule(claim.id, claim.epoch, false)
                                .await
                                .map_err(|_| CoverError::Unavailable)?;
                            if saved {
                                progress.rescheduled += 1;
                            }
                            saved
                        }
                    };
                    changed
                }
            };
            // Another process took the lease, or the DB state changed. Do not
            // acknowledge an operation for a newer lease.
            if !saved {
                return Err(CoverError::LostLease);
            }
        }
        Ok(progress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_work_is_enabled_by_constructing_a_provider() {
        fn assert_provider<T: CdnPurgeProvider>() {}
        assert_provider::<YandexCdnPurge>();
        assert_eq!(CdnPurgeProgress::default().claimed, 0);
    }
}
