//! C-06 / S3-18: private paid chapter grant orchestration.
//!
//! D authorizes exactly one historical version; B resolves that version,
//! and G signs only the matching attached ciphertext object. No plaintext,
//! keys, signed URLs or permission decisions are cached or logged here.
//! This service is NOT wired to an HTTP route until CSRF, Origin, rate
//! limiting, private S3 and the protected browser are verified end-to-end.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{Duration as ChronoDuration, SecondsFormat, Utc};
use tor_contracts::paid_delivery::{
    Actor, ChapterVersionPort, ContentCodec, DeliveryError, DeliveryGrant, DenialReason,
    EncryptedObjectKind, EntitlementDecision, EntitlementPort, GrantedContent, PositiveId,
    PrivateObjectDeliveryPort, DEFAULT_SIGNED_GET_TTL,
};

fn denied(reason: DenialReason) -> DeliveryError {
    match reason {
        DenialReason::Hidden => DeliveryError::Hidden,
        DenialReason::Unknown => DeliveryError::Unavailable,
        DenialReason::NotPurchased | DenialReason::Refunded | DenialReason::Locked => {
            DeliveryError::Denied
        }
    }
}

/// Only a server-authenticated actor and optional owned purchase identity
/// reach this service. Neither a version ID nor an object key can be supplied
/// by the HTTP body. Both the initial and final D resolutions must agree.
pub async fn issue_paid_chapter_grant(
    entitlement: &dyn EntitlementPort,
    versions: &dyn ChapterVersionPort,
    storage: &dyn PrivateObjectDeliveryPort,
    actor: Actor,
    chapter_id: PositiveId,
    purchase_id: Option<PositiveId>,
) -> Result<DeliveryGrant, DeliveryError> {
    let pinned = match entitlement
        .resolve_entitlement(actor, chapter_id, purchase_id)
        .await?
    {
        EntitlementDecision::Allowed { pinned } => pinned,
        EntitlementDecision::Denied { reason } => return Err(denied(reason)),
    };
    if pinned.chapter_id != chapter_id {
        return Err(DeliveryError::Conflict);
    }

    let encrypted = versions
        .encrypted_chapter(pinned)
        .await?
        .ok_or(DeliveryError::Conflict)?;
    if encrypted.object.kind != EncryptedObjectKind::Chapter
        || encrypted.codec != ContentCodec::Gzip
        || encrypted.wrapped_key_id.is_empty()
        || encrypted.aad.is_empty()
        || encrypted.object.size_bytes > 16 * 1024 * 1024
    {
        return Err(DeliveryError::Unavailable);
    }

    // Start the promised expiry before the signing request. This timestamp
    // cannot claim the URL lives longer than its 90s S3 presign period.
    let issued_at = Utc::now();
    let signed = storage
        .grant_verified_object(&encrypted, DEFAULT_SIGNED_GET_TTL)
        .await?;

    // Even though G is a trusted port, reject an incorrectly assembled
    // cross-language grant instead of sending a mismatched hash/nonce/AAD
    // to the browser (which would fail only after fetching ciphertext).
    verify_granted_content(&encrypted, &signed)?;

    // A refund or administrative hold may commit while B/G are queried.
    // Re-check D before returning even an already computed bearer URL/DEK.
    // The remaining transaction race is intrinsic to pre-signed URLs;
    // previously issued URLs cannot be revoked before their expiry.
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

    let expires_at = (issued_at + ChronoDuration::seconds(DEFAULT_SIGNED_GET_TTL.as_secs() as i64))
        .to_rfc3339_opts(SecondsFormat::Secs, true);
    Ok(DeliveryGrant::for_pinned_version(
        pinned,
        expires_at,
        signed,
        Vec::new(),
    ))
}

fn verify_granted_content(
    encrypted: &tor_contracts::paid_delivery::EncryptedObjectRef,
    signed: &GrantedContent,
) -> Result<(), DeliveryError> {
    let expected_hash = hex::encode(encrypted.object.sha256_ciphertext);
    if signed.object_sha256 != expected_hash
        || signed.cipher != "AES-256-GCM"
        || signed.codec != "gzip"
        || signed.nonce_b64u != URL_SAFE_NO_PAD.encode(encrypted.nonce)
        || signed.aad_b64u != URL_SAFE_NO_PAD.encode(&encrypted.aad)
    {
        return Err(DeliveryError::Unavailable);
    }
    Ok(())
}
