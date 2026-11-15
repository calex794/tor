//! One use case for native administrative forms. SQL and live authority stay atomic.
use crate::domain::editorial::*;
use async_trait::async_trait;
use tor_contracts::exposure::PromotionSlot;

#[async_trait]
pub trait EditorialStore: Send + Sync {
    async fn apply(
        &self,
        administrator: &Administrator,
        command_key: &str,
        command: &NormalizedCommand,
    ) -> Result<Receipt, EditorialError>;
    async fn campaigns(&self, after: Option<i64>) -> Result<CampaignPage, EditorialError>;
    async fn cards(
        &self,
        viewer: Option<&Viewer>,
        slot: PromotionSlot,
        host: Option<i64>,
    ) -> Result<Vec<EditorialCard>, EditorialError>;
}

pub struct Editorial<'a> {
    store: &'a dyn EditorialStore,
}
impl<'a> Editorial<'a> {
    pub fn new(store: &'a dyn EditorialStore) -> Self {
        Self { store }
    }
    pub async fn execute(
        &self,
        administrator: &Administrator,
        command_key: &str,
        command: &Command,
    ) -> Result<Receipt, EditorialError> {
        if administrator.user_id <= 0 || !valid_command_key(command_key) {
            return Err(EditorialError::Invalid);
        }
        self.store
            .apply(administrator, command_key, &command.normalize()?)
            .await
    }
    pub async fn campaigns(&self, after: Option<i64>) -> Result<CampaignPage, EditorialError> {
        if after.is_some_and(|id| id <= 0) {
            return Err(EditorialError::Invalid);
        }
        self.store.campaigns(after).await
    }
    pub async fn cards(
        &self,
        viewer: Option<&Viewer>,
        slot: PromotionSlot,
        host: Option<i64>,
    ) -> Result<Vec<EditorialCard>, EditorialError> {
        if viewer.is_some_and(|viewer| viewer.user_id <= 0)
            || host.is_some_and(|id| id <= 0)
            || (slot != PromotionSlot::HomeFeatured && host.is_none())
        {
            return Err(EditorialError::Invalid);
        }
        self.store.cards(viewer, slot, host).await
    }
}
