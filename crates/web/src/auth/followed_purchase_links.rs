//! D-owned purchase lookup for items in the current private C-16 page.
//! Never infer entitlements from Follow, a reader position, or the public TOC.
use super::AuthState;
use tor_commerce::repositories::entitlements::{PgDirectEntitlements, ReadablePurchaseLink};
use tor_contracts::paid_delivery::{DeliveryError, PositiveId};
use tor_reading::repositories::follow_updates::FollowedReleasePage;

pub(super) async fn for_reader(
    state: &AuthState,
    actor: i64,
    updates: &FollowedReleasePage,
) -> Result<Vec<ReadablePurchaseLink>, DeliveryError> {
    let actor = PositiveId::new(actor).map_err(|_| DeliveryError::Denied)?;
    let ids: Vec<i64> = updates
        .items
        .iter()
        .filter(|item| item.is_paid)
        .map(|item| item.chapter_id)
        .collect();
    PgDirectEntitlements::new(state.accounts.pool().clone())
        .readable_purchase_links(actor, &ids)
        .await
}
