//! G S3-20: only an attached PG ciphertext receipt can produce a signed
//! 90-second grant. Real S3 signature behavior is tested in s3_private_provider.
#![cfg(feature = "pg")]

use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use flate2::read::GzDecoder;
use sqlx::PgPool;
use std::{
    io::Read,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tor_contracts::paid_delivery::{
    ChapterVersionRef, ContentCodec, DeliveryError, EncryptedObjectKind, EncryptedObjectRef,
    ObjectId, ObjectRef, PositiveId, PrivateObjectDeliveryPort, SecretValue,
};
use tor_storage::{
    domain::provider::StorageError,
    repositories::{delivery::PgPrivateDelivery, registry::PgStorageRegistry},
    services::{
        crypto::{seal_chapter, sha256_bytes, ContentKek},
        provider::{upload_and_verify, PrivateObjectStore},
    },
};
use zeroize::Zeroizing;

const BUCKET: &str = "tor-private-chapter-ci";

#[derive(Clone, Default)]
struct FakeStore {
    bytes: Arc<Mutex<Option<Vec<u8>>>>,
    healthy: Arc<AtomicBool>,
    signed: Arc<AtomicUsize>,
}

impl FakeStore {
    fn operational() -> Self {
        Self {
            healthy: Arc::new(AtomicBool::new(true)),
            ..Self::default()
        }
    }

    fn signed(&self) -> usize {
        self.signed.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl PrivateObjectStore for FakeStore {
    async fn put_immutable(&self, object: &ObjectRef, bytes: &[u8]) -> Result<(), StorageError> {
        if bytes.len() as u64 != object.size_bytes
            || sha256_bytes(bytes) != object.sha256_ciphertext
        {
            return Err(StorageError::Invalid);
        }
        let mut slot = self.bytes.lock().unwrap();
        if slot.is_some() {
            return Err(StorageError::Conflict);
        }
        *slot = Some(bytes.to_vec());
        Ok(())
    }

    async fn head_verified(&self, object: &ObjectRef) -> Result<bool, StorageError> {
        if !self.healthy.load(Ordering::SeqCst) {
            return Err(StorageError::Unavailable);
        }
        Ok(self.bytes.lock().unwrap().as_deref().is_some_and(|bytes| {
            bytes.len() as u64 == object.size_bytes
                && sha256_bytes(bytes) == object.sha256_ciphertext
        }))
    }

    async fn get_verified(&self, object: &ObjectRef) -> Result<Vec<u8>, StorageError> {
        let bytes = self
            .bytes
            .lock()
            .unwrap()
            .clone()
            .ok_or(StorageError::Unavailable)?;
        if bytes.len() as u64 != object.size_bytes
            || sha256_bytes(&bytes) != object.sha256_ciphertext
        {
            return Err(StorageError::Corrupt);
        }
        Ok(bytes)
    }

    async fn presign_verified(
        &self,
        object: &ObjectRef,
        ttl: Duration,
    ) -> Result<SecretValue, StorageError> {
        if ttl != Duration::from_secs(90) || !self.head_verified(object).await? {
            return Err(StorageError::Unavailable);
        }
        self.signed.fetch_add(1, Ordering::SeqCst);
        SecretValue::new(format!(
            "https://private.invalid/test/{}.enc?signature=TEST-ONLY",
            object.object_id.as_str()
        ))
        .map_err(|_| StorageError::Unavailable)
    }
}

fn version() -> ChapterVersionRef {
    ChapterVersionRef {
        chapter_id: PositiveId::new(101).unwrap(),
        version_id: PositiveId::new(201).unwrap(),
    }
}

fn object_id() -> ObjectId {
    ObjectId::parse("4de6c014-1e2b-440c-bfed-e19721912020").unwrap()
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL; no production credentials"]
async fn g_s3_20_attached_receipt_grant_and_dek_are_exact_and_redacted() {
    let url = std::env::var("TEST_DATABASE_URL").expect("CI database");
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let pool = PgPool::connect(&url).await.unwrap();
    let registry = PgStorageRegistry::new(pool.clone());
    let kek = ContentKek::from_bytes([93; 32]);
    let markdown = "Закрытая купленная V1: А😀Б\n\n第二章";
    let sealed = seal_chapter(version(), object_id(), markdown, &kek).unwrap();
    let object = registry
        .reserve_pending(&sealed, &kek, BUCKET)
        .await
        .unwrap();
    let metadata = EncryptedObjectRef {
        object: object.clone(),
        nonce: sealed.object_nonce,
        aad: sealed.aad.clone(),
        wrapped_key_id: sealed.kek_id.to_owned(),
        codec: ContentCodec::Gzip,
    };
    let store = FakeStore::operational();
    let signer =
        PgPrivateDelivery::new(pool.clone(), store.clone(), kek, BUCKET.to_owned()).unwrap();
    let ttl = Duration::from_secs(90);

    // A newly created registry row is neither verified nor B-attached.
    assert!(matches!(
        signer.grant_verified_object(&metadata, ttl).await,
        Err(DeliveryError::Hidden)
    ));
    assert_eq!(store.signed(), 0);
    let verified = upload_and_verify(&store, &object, &sealed.ciphertext)
        .await
        .unwrap();
    registry.mark_ready(&verified).await.unwrap();
    assert!(matches!(
        signer.grant_verified_object(&metadata, ttl).await,
        Err(DeliveryError::Hidden)
    ));
    sqlx::query(
        "UPDATE storage_objects SET state='attached',attached_at=now(),
                delete_after=NULL WHERE id=$1::uuid",
    )
    .bind(object_id().as_str())
    .execute(&pool)
    .await
    .unwrap();

    for invalid_ttl in [
        Duration::ZERO,
        Duration::from_secs(29),
        Duration::from_secs(121),
    ] {
        assert!(matches!(
            signer.grant_verified_object(&metadata, invalid_ttl).await,
            Err(DeliveryError::Invalid)
        ));
    }
    assert_eq!(store.signed(), 0);

    let mut mismatched = metadata.clone();
    mismatched.nonce[0] ^= 1;
    assert!(matches!(
        signer.grant_verified_object(&mismatched, ttl).await,
        Err(DeliveryError::Unavailable)
    ));
    mismatched = metadata.clone();
    mismatched.aad[0] ^= 1;
    assert!(matches!(
        signer.grant_verified_object(&mismatched, ttl).await,
        Err(DeliveryError::Unavailable)
    ));
    mismatched = metadata.clone();
    mismatched.object.sha256_ciphertext[0] ^= 1;
    assert!(matches!(
        signer.grant_verified_object(&mismatched, ttl).await,
        Err(DeliveryError::Unavailable)
    ));
    assert_eq!(store.signed(), 0);

    // Registry contains a wrapped DEK, not a usable key for an arbitrary KEK.
    let wrong_kek = PgPrivateDelivery::new(
        pool.clone(),
        store.clone(),
        ContentKek::from_bytes([94; 32]),
        BUCKET.to_owned(),
    )
    .unwrap();
    assert!(matches!(
        wrong_kek.grant_verified_object(&metadata, ttl).await,
        Err(DeliveryError::Unavailable)
    ));
    assert_eq!(store.signed(), 0);

    let different_bucket = PgPrivateDelivery::new(
        pool.clone(),
        store.clone(),
        ContentKek::from_bytes([93; 32]),
        "tor-different-bucket".to_owned(),
    )
    .unwrap();
    assert!(matches!(
        different_bucket.grant_verified_object(&metadata, ttl).await,
        Err(DeliveryError::Unavailable)
    ));
    assert_eq!(store.signed(), 0);

    let granted = signer.grant_verified_object(&metadata, ttl).await.unwrap();
    assert_eq!(store.signed(), 1);
    assert_eq!(granted.cipher, "AES-256-GCM");
    assert_eq!(granted.codec, "gzip");
    assert_eq!(granted.object_sha256, hex::encode(object.sha256_ciphertext));
    let encoded = serde_json::to_value(&granted).unwrap();
    let key = Zeroizing::new(
        URL_SAFE_NO_PAD
            .decode(encoded["dek_b64u"].as_str().unwrap())
            .unwrap(),
    );
    assert_eq!(key.len(), 32);
    let bytes = store.get_verified(&object).await.unwrap();
    let clear_gzip = Aes256Gcm::new_from_slice(&key)
        .unwrap()
        .decrypt(
            Nonce::from_slice(&metadata.nonce),
            Payload {
                msg: &bytes,
                aad: &metadata.aad,
            },
        )
        .unwrap();
    let mut inflater = GzDecoder::new(clear_gzip.as_slice());
    let mut plaintext = String::new();
    inflater.read_to_string(&mut plaintext).unwrap();
    assert_eq!(plaintext, markdown);
    assert!(!format!("{granted:?}").contains("signature=TEST-ONLY"));
    assert!(!format!("{granted:?}").contains(encoded["dek_b64u"].as_str().unwrap()));
    assert!(!format!("{granted:?}").contains(markdown));

    store.healthy.store(false, Ordering::SeqCst);
    assert!(matches!(
        signer.grant_verified_object(&metadata, ttl).await,
        Err(DeliveryError::Unavailable)
    ));
    assert_eq!(store.signed(), 1);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn g_s3_20_invalid_bucket_or_unknown_object_never_signs() {
    let url = std::env::var("TEST_DATABASE_URL").expect("CI database");
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let pool = PgPool::connect(&url).await.unwrap();
    let store = FakeStore::operational();
    assert!(matches!(
        PgPrivateDelivery::new(
            pool.clone(),
            store.clone(),
            ContentKek::from_bytes([94; 32]),
            "public-chapters".to_owned()
        ),
        Err(DeliveryError::Invalid)
    ));
    let other_bucket = PgPrivateDelivery::new(
        pool.clone(),
        store.clone(),
        ContentKek::from_bytes([94; 32]),
        "tor-different-bucket".to_owned(),
    )
    .unwrap();
    let unknown_id = ObjectId::parse("88888888-8888-4888-8888-888888888888").unwrap();
    let object = ObjectRef::new(unknown_id, EncryptedObjectKind::Chapter, [1; 32], 64).unwrap();
    let metadata = EncryptedObjectRef {
        object,
        nonce: [2; 12],
        aad:
            b"tor.encrypted.chapter.v1\n101\n201\n4de6c014-1e2b-440c-bfed-e19721912020\n00000000\n"
                .to_vec(),
        wrapped_key_id: "content-kek-v1".to_owned(),
        codec: ContentCodec::Gzip,
    };
    assert!(matches!(
        other_bucket
            .grant_verified_object(&metadata, Duration::from_secs(90))
            .await,
        Err(DeliveryError::Hidden)
    ));
    assert_eq!(store.signed(), 0);
}
