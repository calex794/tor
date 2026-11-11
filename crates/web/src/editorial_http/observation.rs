//! Shared observations use the same delivery flag; historical reports remain readable.
use async_trait::async_trait;
use std::sync::Arc;
use tor_accounts::repositories::PgAccounts;
use tor_ads::repositories::editorial::PgEditorial;
use tor_contracts::exposure::{CardObservation, EditorialObservationGuard, ObservationGuardError};

pub(crate) fn guard(accounts: &PgAccounts) -> Arc<dyn EditorialObservationGuard> {
    Arc::new(Policy {
        store: PgEditorial::new(accounts.pool().clone()),
        enabled: super::enabled(),
    })
}
struct Policy {
    store: PgEditorial,
    enabled: bool,
}
#[async_trait]
impl EditorialObservationGuard for Policy {
    async fn allowed(&self, card: &CardObservation) -> Result<bool, ObservationGuardError> {
        if !self.enabled {
            return Ok(false);
        }
        self.store
            .valid_observation(card)
            .await
            .map_err(|_| ObservationGuardError::Unavailable)
    }
    async fn campaign_book(&self, campaign_id: i64) -> Result<Option<i64>, ObservationGuardError> {
        self.store
            .assigned_book(campaign_id)
            .await
            .map_err(|_| ObservationGuardError::Unavailable)
    }
}
