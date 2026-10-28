//! C S065: D -> B immutable edition/image -> G -> D, no image
//! plaintext, arbitrary keys, browser-selected edition or reusable access.
use super::*;
use chrono::{Duration as ChronoDuration, SecondsFormat, Utc};
use tor_contracts::paid_delivery::{AssetDeliveryGrant, ChapterAssetPort, ObjectId};

/// An image URL is issued ONLY for the version returned by D. The caller can
/// name an object UUID but cannot turn it into a B membership or a D right.
/// A refund/hold during G signing is always checked a second time.
pub async fn issue_paid_asset_grant(
    entitlement: &dyn EntitlementPort,
    assets: &dyn ChapterAssetPort,
    storage: &dyn PrivateObjectDeliveryPort,
    actor: Actor,
    chapter_id: PositiveId,
    purchase_id: Option<PositiveId>,
    object_id: &ObjectId,
) -> Result<AssetDeliveryGrant, DeliveryError> {
    let pinned = match entitlement
        .resolve_entitlement(actor, chapter_id, purchase_id)
        .await?
    {
        EntitlementDecision::Allowed { pinned } if pinned.chapter_id == chapter_id => pinned,
        EntitlementDecision::Allowed { .. } => return Err(DeliveryError::Conflict),
        EntitlementDecision::Denied { reason } => return Err(denied(reason)),
    };
    let encrypted = assets
        .encrypted_asset(pinned, object_id)
        .await?
        .ok_or(DeliveryError::Hidden)?;
    if encrypted.object.object_id.as_str() != object_id.as_str()
        || encrypted.object.kind != EncryptedObjectKind::ChapterAsset
        || encrypted.codec != ContentCodec::None
        || encrypted.wrapped_key_id.is_empty()
        || encrypted.aad.is_empty()
        || encrypted.object.size_bytes > 8 * 1024 * 1024 + 16
    {
        return Err(DeliveryError::Unavailable);
    }
    let issued_at = Utc::now();
    let signed = storage
        .grant_verified_object(&encrypted, DEFAULT_SIGNED_GET_TTL)
        .await?;
    verify_granted_content(&encrypted, &signed)?;

    match entitlement
        .resolve_entitlement(actor, chapter_id, purchase_id)
        .await?
    {
        EntitlementDecision::Allowed {
            pinned: still_allowed,
        } if still_allowed == pinned => {}
        EntitlementDecision::Allowed { .. } => return Err(DeliveryError::Conflict),
        EntitlementDecision::Denied { reason } => return Err(denied(reason)),
    }
    let expiry = (issued_at + ChronoDuration::seconds(DEFAULT_SIGNED_GET_TTL.as_secs() as i64))
        .to_rfc3339_opts(SecondsFormat::Secs, true);
    Ok(AssetDeliveryGrant::for_pinned_asset(
        pinned, object_id, expiry, signed,
    ))
}
