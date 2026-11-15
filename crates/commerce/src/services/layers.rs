//! D-05: business-facing layer operations; SQL and money authority stay in D repository.
use crate::{
    domain::layers::{
        LayerBookCandidatePage, LayerCheckoutCommand, LayerCreateReceipt, LayerCurrentBookPage,
        LayerError, LayerOfferQuote, LayerOwnerOfferState, LayerReceipt, OwnedLayerSummary,
    },
    repositories::{
        layer_composition::{LayerBookTilePage, LayerChangePreview, LayerTileQuery},
        layer_composition_drafts::LayerCompositionDraft,
        layers::PgLayers,
    },
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
    pub async fn author_has_book(&self, actor: PositiveId) -> Result<bool, LayerError> {
        self.repository.author_has_book(actor).await
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
    pub async fn owner_layer(
        &self,
        actor: PositiveId,
        layer: PositiveId,
    ) -> Result<OwnedLayerSummary, LayerError> {
        self.repository.owner_layer(actor, layer).await
    }

    /// Owner-scoped CAS of display metadata only; no changes to paid terms.
    pub async fn update_metadata_if_current(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        expected_revision: i64,
        title: &str,
        description: &str,
        status: &str,
    ) -> Result<(), LayerError> {
        self.repository
            .update_metadata_if_current(actor, layer, expected_revision, title, description, status)
            .await
    }

    pub async fn owner_layer_status(
        &self,
        actor: PositiveId,
        layer: PositiveId,
    ) -> Result<String, LayerError> {
        self.repository.owner_layer_status(actor, layer).await
    }

    /// The owner's current composition with a version for CAS removal.
    pub async fn current_books(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        after: Option<PositiveId>,
        limit: u16,
    ) -> Result<LayerCurrentBookPage, LayerError> {
        self.repository
            .current_books(actor, layer, after, limit)
            .await
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
    pub async fn addable_books_matching(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        after: Option<PositiveId>,
        limit: u16,
        query: &str,
    ) -> Result<LayerBookCandidatePage, LayerError> {
        self.repository
            .addable_books_matching(actor, layer, after, limit, query)
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
    /// Browser form removal rejects stale composition revisions.
    pub async fn remove_book_if_current(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        book: PositiveId,
        expected_version: PositiveId,
    ) -> Result<i64, LayerError> {
        self.repository
            .remove_book_if_current(actor, layer, book, expected_version)
            .await
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
    /// Read current offer only after checking verified owner identity in PG.
    pub async fn owner_offer_state(
        &self,
        actor: PositiveId,
        layer: PositiveId,
    ) -> Result<LayerOwnerOfferState, LayerError> {
        self.repository.owner_offer_state(actor, layer).await
    }

    pub async fn composition_tiles(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        after: Option<PositiveId>,
        search: &str,
        limit: u16,
    ) -> Result<LayerBookTilePage, LayerError> {
        self.repository
            .composition_tiles(actor, layer, after, search, limit)
            .await
    }

    pub async fn composition_draft(
        &self,
        actor: PositiveId,
        layer: PositiveId,
    ) -> Result<LayerCompositionDraft, LayerError> {
        self.repository.composition_draft(actor, layer).await
    }

    pub async fn composition_tiles_filtered(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        query: &LayerTileQuery<'_>,
        draft: &LayerCompositionDraft,
    ) -> Result<LayerBookTilePage, LayerError> {
        self.repository
            .composition_tiles_filtered(actor, layer, query, &draft.additions, &draft.removals)
            .await
    }

    pub async fn stage_composition_page(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        version: PositiveId,
        revision: i64,
        visible: &[i64],
        selected: &[i64],
    ) -> Result<LayerCompositionDraft, LayerError> {
        self.repository
            .stage_composition_page(actor, layer, version, revision, visible, selected)
            .await
    }

    pub async fn discard_composition_draft(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        revision: i64,
    ) -> Result<(), LayerError> {
        self.repository
            .discard_composition_draft(actor, layer, revision)
            .await
    }

    pub async fn discard_stale_composition_draft(
        &self,
        actor: PositiveId,
        layer: PositiveId,
    ) -> Result<(), LayerError> {
        self.repository
            .discard_stale_composition_draft(actor, layer)
            .await
    }

    pub async fn apply_composition_draft(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        version: PositiveId,
        revision: i64,
        expected_buyers: (i64, i64),
    ) -> Result<i64, LayerError> {
        self.repository
            .apply_composition_draft(actor, layer, version, revision, expected_buyers)
            .await
    }

    pub async fn preview_composition(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        version: PositiveId,
        additions: &[i64],
        removals: &[i64],
    ) -> Result<LayerChangePreview, LayerError> {
        self.repository
            .preview_composition(actor, layer, version, additions, removals)
            .await
    }

    pub async fn apply_composition(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        version: PositiveId,
        additions: &[i64],
        removals: &[i64],
        expected_buyers: (i64, i64),
    ) -> Result<i64, LayerError> {
        self.repository
            .apply_composition(actor, layer, version, additions, removals, expected_buyers)
            .await
    }

    /// A browser change checks the original offer and composition under lock.
    pub async fn offer_if_current(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        expected_version: PositiveId,
        expected_offer: Option<PositiveId>,
        price_minor: i64,
        lifetime: bool,
    ) -> Result<i64, LayerError> {
        self.repository
            .publish_offer_if_current(
                actor,
                layer,
                expected_version,
                expected_offer,
                price_minor,
                lifetime,
            )
            .await
    }

    /// Advisory current offer; purchase uses a separate locked D preflight.
    /// Owner-only cosmetic selection. Paid versions and offers remain unchanged.
    pub async fn choose_package_cover(
        &self,
        actor: PositiveId,
        layer: PositiveId,
        book: Option<PositiveId>,
        expected_revision: i64,
    ) -> Result<(), LayerError> {
        self.repository
            .choose_package_cover(actor, layer, book, expected_revision)
            .await
    }

    pub async fn selected_package_cover(
        &self,
        actor: PositiveId,
        layer: PositiveId,
    ) -> Result<Option<i64>, LayerError> {
        self.repository.selected_package_cover(actor, layer).await
    }

    pub async fn quote(&self, layer: PositiveId) -> Result<LayerOfferQuote, LayerError> {
        self.repository.offer_quote(layer).await
    }
    pub async fn purchase(&self, cmd: &LayerCheckoutCommand) -> Result<LayerReceipt, LayerError> {
        self.repository.buy(cmd).await
    }
    pub async fn has_book(&self, actor: PositiveId, book: PositiveId) -> Result<bool, LayerError> {
        self.repository.can_read_book(actor, book).await
    }
}
