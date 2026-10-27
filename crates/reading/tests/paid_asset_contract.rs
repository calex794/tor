//! C/S065: unit contract for D -> B pinned image -> G -> D.
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use std::{sync::{atomic::{AtomicBool, AtomicUsize, Ordering}, Arc}, time::Duration};
use tor_contracts::paid_delivery::{
    Actor, AssetDeliveryGrant, ChapterAssetPort, ChapterVersionRef, ContentCodec,
    DeliveryError, DenialReason, EncryptedObjectKind, EncryptedObjectRef,
    EntitlementDecision, EntitlementPort, GrantedContent, ObjectId, ObjectRef,
    PositiveId, PrivateObjectDeliveryPort, SecretValue,
};
use tor_reading::services::paid_delivery::issue_paid_asset_grant;

fn id(value: i64) -> PositiveId {
    PositiveId::new(value).unwrap()
}
fn object() -> ObjectId {
    ObjectId::parse("a5500000-aaaa-4aaa-8aaa-aaaaaaaaaaaa").unwrap()
}
fn pinned() -> ChapterVersionRef {
    ChapterVersionRef { chapter_id: id(20), version_id: id(90) }
}
fn encrypted() -> EncryptedObjectRef {
    EncryptedObjectRef {
        object: ObjectRef::new(object(), EncryptedObjectKind::ChapterAsset, [5; 32], 66).unwrap(),
        nonce: [4; 12],
        aad: b"tor.encrypted.asset.v1\n20\n90\na5500000-aaaa-4aaa-8aaa-aaaaaaaaaaaa\nhash\n".to_vec(),
        wrapped_key_id: "content-kek-v1".to_owned(),
        codec: ContentCodec::None,
    }
}
struct D {
    revoked: Arc<AtomicBool>,
    calls: AtomicUsize,
}
#[async_trait]
impl EntitlementPort for D {
    async fn resolve_entitlement(
        &self,
        _actor: Actor,
        chapter_id: PositiveId,
        _purchase: Option<PositiveId>,
    ) -> Result<EntitlementDecision, DeliveryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(chapter_id, id(20));
        Ok(if self.revoked.load(Ordering::SeqCst) {
            EntitlementDecision::Denied { reason: DenialReason::Refunded }
        } else {
            EntitlementDecision::Allowed { pinned: pinned() }
        })
    }
}
struct B {
    available: bool,
    calls: AtomicUsize,
}
#[async_trait]
impl ChapterAssetPort for B {
    async fn encrypted_asset(
        &self,
        requested: ChapterVersionRef,
        object_id: &ObjectId,
    ) -> Result<Option<EncryptedObjectRef>, DeliveryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if !self.available || requested != pinned() || *object_id != object() {
            return Ok(None);
        }
        Ok(Some(encrypted()))
    }
}
struct G {
    revoked_after_sign: Arc<AtomicBool>,
    tampered_codec: bool,
    calls: AtomicUsize,
}
#[async_trait]
impl PrivateObjectDeliveryPort for G {
    async fn grant_verified_object(
        &self,
        object: &EncryptedObjectRef,
        ttl: Duration,
    ) -> Result<GrantedContent, DeliveryError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(ttl, Duration::from_secs(90));
        assert_eq!(object.object.kind, EncryptedObjectKind::ChapterAsset);
        assert_eq!(object.codec, ContentCodec::None);
        self.revoked_after_sign.store(true, Ordering::SeqCst);
        Ok(GrantedContent {
            ciphertext_url: SecretValue::new("https://private.test/private/chapter-assets/secret.enc?X-Amz-Signature=private".into()).unwrap(),
            object_sha256: hex::encode(object.object.sha256_ciphertext),
            cipher: "AES-256-GCM",
            codec: if self.tampered_codec { "gzip" } else { "none" },
            nonce_b64u: URL_SAFE_NO_PAD.encode(object.nonce),
            aad_b64u: URL_SAFE_NO_PAD.encode(&object.aad),
            dek_b64u: SecretValue::new("NOT_FOR_LOGS".into()).unwrap(),
        })
    }
}

async fn issue(d: &D, b: &B, g: &G, wanted: &ObjectId) -> Result<AssetDeliveryGrant, DeliveryError> {
    issue_paid_asset_grant(
        d, b, g, Actor { user_id: id(14) }, id(20), Some(id(18)), wanted,
    ).await
}

#[tokio::test]
async fn asset_grant_is_pinned_by_d_and_returns_only_scoped_media() {
    let revoked = Arc::new(AtomicBool::new(false));
    let d = D { revoked: revoked.clone(), calls: AtomicUsize::new(0) };
    let b = B { available: true, calls: AtomicUsize::new(0) };
    // For a successful request D remains Allowed through both checks.
    let g = G { revoked_after_sign: Arc::new(AtomicBool::new(false)), tampered_codec: false, calls: AtomicUsize::new(0) };
    let data = issue(&d, &b, &g, &object()).await.unwrap();
    assert_eq!(data.schema, "tor.paid_asset_delivery.v1");
    assert_eq!(data.chapter_id, "20");
    assert_eq!(data.version_id, "90");
    assert_eq!(data.object_id, object().as_str());
    assert_eq!(data.content.codec, "none");
    assert_eq!(d.calls.load(Ordering::SeqCst), 2);
    assert_eq!(b.calls.load(Ordering::SeqCst), 1);
    assert_eq!(g.calls.load(Ordering::SeqCst), 1);
    let diagnostic = format!("{data:?}");
    assert!(!diagnostic.contains("NOT_FOR_LOGS"));
    assert!(!diagnostic.contains("X-Amz-Signature"));
}

#[tokio::test]
async fn missing_edition_membership_refuses_signing_and_refund_discards_result() {
    let revoked = Arc::new(AtomicBool::new(false));
    let d = D { revoked: revoked.clone(), calls: AtomicUsize::new(0) };
    let b = B { available: true, calls: AtomicUsize::new(0) };
    let g = G { revoked_after_sign: revoked.clone(), tampered_codec: false, calls: AtomicUsize::new(0) };
    let other = ObjectId::parse("bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb").unwrap();
    assert!(matches!(issue(&d, &b, &g, &other).await, Err(DeliveryError::Hidden)));
    assert_eq!(g.calls.load(Ordering::SeqCst), 0);
    assert_eq!(d.calls.load(Ordering::SeqCst), 1);

    // G signs between two D lookups, but the refund must discard its bearer.
    assert!(matches!(issue(&d, &b, &g, &object()).await, Err(DeliveryError::Denied)));
    assert_eq!(g.calls.load(Ordering::SeqCst), 1);
    assert!(revoked.load(Ordering::SeqCst));
    assert_eq!(d.calls.load(Ordering::SeqCst), 3);
    assert!(matches!(issue(&d, &b, &g, &object()).await, Err(DeliveryError::Denied)));
    assert_eq!(g.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn G_response_with_wrong_codec_is_never_exposed() {
    let revoked = Arc::new(AtomicBool::new(false));
    let d = D { revoked, calls: AtomicUsize::new(0) };
    let b = B { available: true, calls: AtomicUsize::new(0) };
    let g = G { revoked_after_sign: Arc::new(AtomicBool::new(false)), tampered_codec: true, calls: AtomicUsize::new(0) };
    assert!(matches!(
        issue(&d, &b, &g, &object()).await,
        Err(DeliveryError::Unavailable)
    ));
    assert_eq!(d.calls.load(Ordering::SeqCst), 1);
}
