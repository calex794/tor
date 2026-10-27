use std::time::Duration;
use tor_contracts::paid_delivery::{
    validate_signed_get_ttl, Actor, ChapterVersionRef, ContentCodec, ContractError, DeliveryError,
    DeliveryGrant, DenialReason, EncryptedObjectKind, EntitlementDecision, GrantedContent,
    ObjectId, ObjectRef, PositiveId, SecretValue, DEFAULT_SIGNED_GET_TTL,
};

fn id(value: i64) -> PositiveId {
    PositiveId::new(value).unwrap()
}

fn object_id() -> ObjectId {
    ObjectId::parse("cb62d5a3-3d9e-4327-8dd4-10b6e3eaaf61").unwrap()
}

#[test]
fn ids_are_positive_and_are_decimal_strings_even_beyond_js_precision() {
    assert_eq!(PositiveId::new(0), Err(ContractError::InvalidId));
    assert_eq!(PositiveId::new(-9), Err(ContractError::InvalidId));
    assert_eq!(id(9_007_199_254_740_993).wire(), "9007199254740993");
    let actor = Actor { user_id: id(17) };
    assert_eq!(actor.user_id.get(), 17);
}

#[test]
fn object_ids_reject_paths_uppercase_and_noncanonical_forms() {
    for invalid in [
        "CB62D5A3-3D9E-4327-8DD4-10B6E3EAAF61",
        "cb62d5a3-3d9e-4327-8dd4-10b6e3eaaf61.enc",
        "../../cb62d5a3-3d9e-4327-8dd4-10b6e3eaaf61",
        "cb62d5a33d9e43278dd410b6e3eaaf61",
        "cb62d5a3-3d9e-4327-8dd4-10b6e3eaaf6g",
        "cb62d5a3-3d9e-4327-8dd4-10b6e3eaaf61/another",
        "",
    ] {
        assert_eq!(ObjectId::parse(invalid), Err(ContractError::InvalidObjectId));
    }
    assert_eq!(object_id().as_str(), "cb62d5a3-3d9e-4327-8dd4-10b6e3eaaf61");
}

#[test]
fn unsigned_or_uninitialized_ciphertext_cannot_be_a_trusted_object() {
    assert_eq!(
        ObjectRef::new(object_id(), EncryptedObjectKind::Chapter, [0; 32], 512),
        Err(ContractError::InvalidCiphertext)
    );
    assert_eq!(
        ObjectRef::new(object_id(), EncryptedObjectKind::Chapter, [4; 32], 16),
        Err(ContractError::InvalidCiphertext)
    );
    let valid = ObjectRef::new(
        object_id(),
        EncryptedObjectKind::ChapterAsset,
        [4; 32],
        17,
    )
    .unwrap();
    assert_eq!(valid.size_bytes, 17);
    assert_eq!(ContentCodec::Gzip.wire(), "gzip");
    assert_eq!(ContentCodec::None.wire(), "none");
}

#[test]
fn signed_get_ttl_is_bounded_by_contract() {
    assert_eq!(
        validate_signed_get_ttl(DEFAULT_SIGNED_GET_TTL),
        Ok(Duration::from_secs(90))
    );
    assert_eq!(
        validate_signed_get_ttl(Duration::from_secs(30)),
        Ok(Duration::from_secs(30))
    );
    assert_eq!(
        validate_signed_get_ttl(Duration::from_secs(120)),
        Ok(Duration::from_secs(120))
    );
    for ttl in [Duration::ZERO, Duration::from_secs(29), Duration::from_secs(121)] {
        assert_eq!(validate_signed_get_ttl(ttl), Err(ContractError::InvalidTtl));
    }
}

#[test]
fn denial_and_error_contracts_are_not_entitlements() {
    assert_eq!(
        EntitlementDecision::Denied {
            reason: DenialReason::Refunded
        },
        EntitlementDecision::Denied {
            reason: DenialReason::Refunded
        }
    );
    assert_ne!(DeliveryError::Unavailable, DeliveryError::Denied);
    assert_ne!(DeliveryError::Hidden, DeliveryError::Conflict);
}

#[test]
fn secrets_are_redacted_from_nested_debug_and_wire_ids_are_strings() {
    let url = "https://private.example/opaque?X-Amz-Signature=not-for-logs";
    let key = "scoped-dek-b64u-private";
    let content = GrantedContent {
        ciphertext_url: SecretValue::new(url.to_owned()).unwrap(),
        object_sha256: "ab".repeat(32),
        cipher: "AES-256-GCM",
        codec: "gzip",
        nonce_b64u: "nonce".to_owned(),
        aad_b64u: "aad".to_owned(),
        dek_b64u: SecretValue::new(key.to_owned()).unwrap(),
    };
    let grant = DeliveryGrant::for_pinned_version(
        ChapterVersionRef {
            chapter_id: id(9_007_199_254_740_993),
            version_id: id(106),
        },
        "2026-10-27T12:00:00Z".to_owned(),
        content,
        Vec::new(),
    );
    let diagnostic = format!("{grant:?}");
    assert!(!diagnostic.contains(url));
    assert!(!diagnostic.contains(key));
    assert!(diagnostic.contains("[REDACTED]"));
    let wire = serde_json::to_value(&grant).unwrap();
    assert_eq!(wire["schema"], "tor.paid_delivery.v1");
    assert_eq!(wire["chapter_id"], "9007199254740993");
    assert_eq!(wire["version_id"], "106");
    assert_eq!(wire["protected_mode"], "pua_v1");
    assert_eq!(wire["content"]["ciphertext_url"], url);
    assert_eq!(wire["content"]["dek_b64u"], key);
    assert_eq!(wire["assets"], serde_json::json!([]));
}

#[test]
fn empty_or_oversized_secrets_cannot_be_built() {
    assert_eq!(
        SecretValue::new(String::new()),
        Err(ContractError::InvalidCiphertext)
    );
    assert_eq!(
        SecretValue::new("a".repeat(4097)),
        Err(ContractError::InvalidCiphertext)
    );
}
