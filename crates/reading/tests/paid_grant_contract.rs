//! C-06/S3-18: D→B→G order, pinned V1, revocation race and no-secret failures.
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, Utc};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    },
    time::Duration,
};
use tor_contracts::paid_delivery::{
    Actor, ChapterVersionPort, ChapterVersionRef, ContentCodec, DeliveryError, DenialReason,
    EncryptedObjectKind, EncryptedObjectRef, EntitlementDecision, EntitlementPort, GrantedContent,
    ObjectId, ObjectRef, PositiveId, PrivateObjectDeliveryPort, SecretValue,
    DEFAULT_SIGNED_GET_TTL,
};
use tor_reading::services::paid_delivery::issue_paid_chapter_grant;

fn id(n: i64) -> PositiveId {
    PositiveId::new(n).unwrap()
}
fn actor() -> Actor {
    Actor { user_id: id(33) }
}
fn pin(v: i64) -> ChapterVersionRef {
    ChapterVersionRef {
        chapter_id: id(44),
        version_id: id(v),
    }
}
fn allowed(v: i64) -> Result<EntitlementDecision, DeliveryError> {
    Ok(EntitlementDecision::Allowed { pinned: pin(v) })
}
fn encrypted() -> EncryptedObjectRef {
    EncryptedObjectRef {
        object: ObjectRef::new(
            ObjectId::parse("01234567-89ab-4cde-8fab-0123456789ab").unwrap(),
            EncryptedObjectKind::Chapter,
            [0x5d; 32],
            321,
        )
        .unwrap(),
        nonce: [0x42; 12],
        aad: b"tor.encrypted.chapter.v1\n44\n111\n...".to_vec(),
        wrapped_key_id: "content-kek-v1".to_owned(),
        codec: ContentCodec::Gzip,
    }
}

struct D {
    next: Mutex<VecDeque<Result<EntitlementDecision, DeliveryError>>>,
    count: AtomicUsize,
}
impl D {
    fn new(results: Vec<Result<EntitlementDecision, DeliveryError>>) -> Self {
        Self {
            next: Mutex::new(results.into()),
            count: AtomicUsize::new(0),
        }
    }
    fn calls(&self) -> usize {
        self.count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl EntitlementPort for D {
    async fn resolve_entitlement(
        &self,
        actor: Actor,
        chapter_id: PositiveId,
        purchase_id: Option<PositiveId>,
    ) -> Result<EntitlementDecision, DeliveryError> {
        assert_eq!(actor.user_id, id(33));
        assert_eq!(chapter_id, id(44));
        assert_eq!(purchase_id, Some(id(71)));
        self.count.fetch_add(1, Ordering::SeqCst);
        self.next
            .lock()
            .unwrap()
            .pop_front()
            .expect("one D response per call")
    }
}

struct B {
    result: Option<EncryptedObjectRef>,
    requested: Mutex<Vec<ChapterVersionRef>>,
}
impl B {
    fn new(result: Option<EncryptedObjectRef>) -> Self {
        Self {
            result,
            requested: Mutex::new(Vec::new()),
        }
    }
    fn calls(&self) -> Vec<ChapterVersionRef> {
        self.requested.lock().unwrap().clone()
    }
}

#[async_trait]
impl ChapterVersionPort for B {
    async fn encrypted_chapter(
        &self,
        pinned: ChapterVersionRef,
    ) -> Result<Option<EncryptedObjectRef>, DeliveryError> {
        self.requested.lock().unwrap().push(pinned);
        Ok(self.result.clone())
    }
}

#[derive(Default)]
struct G {
    calls: Mutex<Vec<(ObjectId, Duration)>>,
    unavailable: bool,
    wrong_sha: bool,
    wrong_nonce: bool,
}
impl G {
    fn count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}
#[async_trait]
impl PrivateObjectDeliveryPort for G {
    async fn grant_verified_object(
        &self,
        encrypted: &EncryptedObjectRef,
        ttl: Duration,
    ) -> Result<GrantedContent, DeliveryError> {
        self.calls
            .lock()
            .unwrap()
            .push((encrypted.object.object_id.clone(), ttl));
        if self.unavailable {
            return Err(DeliveryError::Unavailable);
        }
        Ok(GrantedContent {
            ciphertext_url: SecretValue::new(
                "https://private-storage.example.invalid/V1?X-Amz-Signature=SENSITIVE".to_owned(),
            )
            .unwrap(),
            object_sha256: if self.wrong_sha {
                "00".repeat(32)
            } else {
                hex::encode(encrypted.object.sha256_ciphertext)
            },
            cipher: "AES-256-GCM",
            codec: encrypted.codec.wire(),
            nonce_b64u: if self.wrong_nonce {
                URL_SAFE_NO_PAD.encode([99; 12])
            } else {
                URL_SAFE_NO_PAD.encode(encrypted.nonce)
            },
            aad_b64u: URL_SAFE_NO_PAD.encode(&encrypted.aad),
            dek_b64u: SecretValue::new("SENSITIVE-SCOPED-DEK".to_owned()).unwrap(),
        })
    }
}

#[tokio::test]
async fn c_06_paid_v1_grant_uses_only_d_pin_and_never_logs_secrets() {
    let d = D::new(vec![allowed(111), allowed(111)]);
    let b = B::new(Some(encrypted()));
    let g = G::default();
    let before = Utc::now();
    let result = issue_paid_chapter_grant(&d, &b, &g, actor(), id(44), Some(id(71)))
        .await
        .unwrap();
    assert_eq!(d.calls(), 2, "re-check after S3 presign");
    assert_eq!(b.calls(), vec![pin(111)]);
    assert_eq!(g.count(), 1);
    assert_eq!(g.calls.lock().unwrap()[0].1, DEFAULT_SIGNED_GET_TTL);
    assert_eq!(result.chapter_id, "44");
    assert_eq!(result.version_id, "111", "never current V2");
    assert!(result.assets.is_empty());
    let timestamp = DateTime::parse_from_rfc3339(&result.expires_at).unwrap();
    let lifespan = timestamp.signed_duration_since(before);
    assert!((89..=91).contains(&lifespan.num_seconds()));
    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["schema"], "tor.paid_delivery.v1");
    assert_eq!(json["content"]["codec"], "gzip");
    assert_eq!(json["content"]["object_sha256"], "5d".repeat(32));
    assert!(json["content"]["ciphertext_url"]
        .as_str()
        .unwrap()
        .contains("X-Amz-Signature="));
    let debug = format!("{result:?}");
    assert!(
        !debug.contains("SENSITIVE"),
        "never leak URL or DEK in Debug"
    );
}

#[tokio::test]
async fn c_06_denials_do_not_consult_b_or_g_or_return_secrets() {
    for (reason, expected) in [
        (DenialReason::NotPurchased, DeliveryError::Denied),
        (DenialReason::Refunded, DeliveryError::Denied),
        (DenialReason::Locked, DeliveryError::Denied),
        (DenialReason::Hidden, DeliveryError::Hidden),
        (DenialReason::Unknown, DeliveryError::Unavailable),
    ] {
        let d = D::new(vec![Ok(EntitlementDecision::Denied { reason })]);
        let b = B::new(Some(encrypted()));
        let g = G::default();
        let reply = issue_paid_chapter_grant(&d, &b, &g, actor(), id(44), Some(id(71))).await;
        assert_eq!(reply.unwrap_err(), expected);
        assert_eq!(d.calls(), 1);
        assert!(b.calls().is_empty(), "cannot look up S3 before D allows");
        assert_eq!(g.count(), 0, "no bearer URL for denied actor");
    }
}

#[tokio::test]
async fn c_06_rechecks_refund_and_version_switch_after_signing() {
    let d = D::new(vec![
        allowed(111),
        Ok(EntitlementDecision::Denied {
            reason: DenialReason::Refunded,
        }),
    ]);
    let b = B::new(Some(encrypted()));
    let g = G::default();
    let refund = issue_paid_chapter_grant(&d, &b, &g, actor(), id(44), Some(id(71))).await;
    assert_eq!(refund.unwrap_err(), DeliveryError::Denied);
    assert_eq!(g.count(), 1, "a generated signed URL is never returned");
    assert_eq!(d.calls(), 2);

    let changed = D::new(vec![allowed(111), allowed(222)]);
    assert_eq!(
        issue_paid_chapter_grant(&changed, &b, &g, actor(), id(44), Some(id(71)))
            .await
            .unwrap_err(),
        DeliveryError::Conflict
    );
}

#[tokio::test]
async fn c_06_mismatched_d_chapter_and_b_version_never_reach_signer() {
    let wrong = D::new(vec![Ok(EntitlementDecision::Allowed {
        pinned: ChapterVersionRef {
            chapter_id: id(99),
            version_id: id(111),
        },
    })]);
    let b = B::new(Some(encrypted()));
    let g = G::default();
    assert_eq!(
        issue_paid_chapter_grant(&wrong, &b, &g, actor(), id(44), Some(id(71)))
            .await
            .unwrap_err(),
        DeliveryError::Conflict
    );
    assert_eq!(g.count(), 0);
    assert!(b.calls().is_empty());

    let d = D::new(vec![allowed(111)]);
    let no_object = B::new(None);
    assert_eq!(
        issue_paid_chapter_grant(&d, &no_object, &g, actor(), id(44), Some(id(71)))
            .await
            .unwrap_err(),
        DeliveryError::Conflict
    );
    assert_eq!(g.count(), 0);
}

#[tokio::test]
async fn c_06_storage_failures_or_mismatched_metadata_fail_closed() {
    let b = B::new(Some(encrypted()));
    for g in [
        G {
            unavailable: true,
            ..G::default()
        },
        G {
            wrong_sha: true,
            ..G::default()
        },
        G {
            wrong_nonce: true,
            ..G::default()
        },
    ] {
        let d = D::new(vec![allowed(111)]);
        let response = issue_paid_chapter_grant(&d, &b, &g, actor(), id(44), Some(id(71))).await;
        assert_eq!(response.unwrap_err(), DeliveryError::Unavailable);
        assert_eq!(d.calls(), 1);
    }
    let mut wrong_kind = encrypted();
    wrong_kind.object.kind = EncryptedObjectKind::ChapterAsset;
    let bad = B::new(Some(wrong_kind));
    let d = D::new(vec![allowed(111)]);
    let g = G::default();
    assert_eq!(
        issue_paid_chapter_grant(&d, &bad, &g, actor(), id(44), Some(id(71)))
            .await
            .unwrap_err(),
        DeliveryError::Unavailable
    );
    assert_eq!(g.count(), 0);
}
