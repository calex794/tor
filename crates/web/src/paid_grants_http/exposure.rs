//! Source-aware first paid-grant evidence; never a separate authorization.
use super::*;
use tor_commerce::repositories::direct_exposure::DirectGrantKind;
use tor_contracts::paid_delivery::DenialReason;

/// Every protected paid grant records the D-owned source actually used.
/// A direct pinned V1 takes priority in the default source path. The Layer
/// fallback is never allowed for an explicitly requested direct purchase.
pub(super) async fn record_paid_grant_exposure(
    state: &GrantState,
    actor: PositiveId,
    chapter: PositiveId,
    purchase: Option<PositiveId>,
    source: Option<GrantSource>,
    version: &str,
    kind: LayerGrantKind,
) -> Result<(), DeliveryError> {
    let version_id = positive(version).ok_or(DeliveryError::Unavailable)?;
    if source.is_none() {
        let direct_kind = match kind {
            LayerGrantKind::Chapter => DirectGrantKind::Chapter,
            LayerGrantKind::Asset => DirectGrantKind::Asset,
        };
        if purchase.is_some() {
            return state
                .first_direct_grant
                .record(actor, chapter, version_id, purchase, direct_kind)
                .await;
        }
        match state
            .direct
            .resolve_entitlement(Actor { user_id: actor }, chapter, None)
            .await?
        {
            EntitlementDecision::Allowed { pinned } if pinned.version_id == version_id => {
                return state
                    .first_direct_grant
                    .record(actor, chapter, version_id, None, direct_kind)
                    .await;
            }
            EntitlementDecision::Allowed { .. } => return Err(DeliveryError::Conflict),
            EntitlementDecision::Denied {
                reason: DenialReason::NotPurchased | DenialReason::Refunded,
            } => {}
            EntitlementDecision::Denied {
                reason: DenialReason::Hidden,
            } => return Err(DeliveryError::Hidden),
            EntitlementDecision::Denied {
                reason: DenialReason::Unknown,
            } => return Err(DeliveryError::Unavailable),
            EntitlementDecision::Denied { .. } => return Err(DeliveryError::Denied),
        }
    }
    state
        .first_layer_grant
        .record(actor, chapter, version_id, kind)
        .await
}
