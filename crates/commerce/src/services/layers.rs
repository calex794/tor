//! D-05: business-facing layer operations; SQL and money authority stay in D repository.
use crate::{
    domain::layers::{
        LayerBookCandidatePage, LayerCheckoutCommand, LayerCreateReceipt, LayerError, LayerReceipt,
        OwnedLayerSummary,
    },
    repositories::layers::PgLayers,
};
use tor_contracts::paid_delivery::PositiveId;

#[derive(Clone)]
pub struct LayerService {
    repository: PgLayers,
}
impl LayerService {
    pub fn new(repository: PgLayers) -> Self {
        Self { repository }
    }
    pub async fn create(&self, actor: PositiveId, title: &str) -> Result<i64, LayerError> {
        self.repository.create_layer(actor, title).await
    }
    /// Authenticated form command. The original Layer is returned on replay.
    pub async fn create_once(
        &self,
        actor: PositiveId,
        title: &str,
        key: &crate::domain::money::IdempotencyKey,
    ) -> Result<LayerCreateReceipt, LayerError> {
        self.repository.create_layer_once(actor, title, key).await
    }
    /// Show only this owner's Layer containers in the author workspace.
    pub async fn owned_layers(
        &self,
        actor: PositiveId,
        limit: u16,
    ) -> Result<Vec<OwnedLayerSummary>, LayerError> {
        self.repository.owned_layers(actor, limit).await
    }
    /// UI list is advisory; the write performs a locked recheck.
    pub async fn addable_books(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        after: Option<PositiveId>,
        limit: u16,
    ) -> Result<LayerBookCandidatePage, LayerError> {
        self.repository
            .addable_books(actor, layer, after, limit)
            .await
    }
    pub async fn add_book(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        book: PositiveId,
    ) -> Result<i64, LayerError> {
        self.repository.add_book(actor, layer, book).await
    }
    pub async fn remove_book(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        book: PositiveId,
    ) -> Result<i64, LayerError> {
        self.repository.remove_book(actor, layer, book).await
    }
    pub async fn offer(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        price_minor: i64,
        lifetime: bool,
    ) -> Result<i64, LayerError> {
        self.repository
            .publish_offer(actor, layer, price_minor, lifetime)
            .await
    }
    pub async fn purchase(&self, cmd: &LayerCheckoutCommand) -> Result<LayerReceipt, LayerError> {
        self.repository.buy(cmd).await
    }
    pub async fn has_book(&self, actor: PositiveId, book: PositiveId) -> Result<bool, LayerError> {
        self.repository.can_read_book(actor, book).await
    }
}
