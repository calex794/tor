//! G/S065: private PNG AES-GCM object is signed only for a released B edition.
//! These tests run only against disposable PostgreSQL 18 (never a live VPS).
#![cfg(feature = "pg")]

use aes_gcm::{
    aead::{Aead, Payload},
    Aes256Gcm, KeyInit, Nonce,
};
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sqlx::PgPool;
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicUsize, Ordering},
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
    repositories::{paid_delivery::PgPrivateDelivery, registry::PgStorageRegistry},
    services::{
        crypto::{seal_asset_with_keyring, seal_chapter, ContentKek, ContentKeyring},
        provider::{upload_and_verify, PrivateObjectStore},
    },
};

#[derive(Clone, Default)]
struct PrivateFakeS3 {
    objects: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    signed: Arc<AtomicUsize>,
}

#[async_trait]
impl PrivateObjectStore for PrivateFakeS3 {
    async fn put_immutable(&self, object: &ObjectRef, bytes: &[u8]) -> Result<(), StorageError> {
        let mut stored = self.objects.lock().unwrap();
        if stored.contains_key(object.object_id.as_str()) {
            return Err(StorageError::Conflict);
        }
        stored.insert(object.object_id.as_str().to_owned(), bytes.to_vec());
        Ok(())
    }
    async fn head_verified(&self, object: &ObjectRef) -> Result<bool, StorageError> {
        Ok(self
            .objects
            .lock()
            .unwrap()
            .get(object.object_id.as_str())
            .is_some_and(|bytes| bytes.len() as u64 == object.size_bytes))
    }
    async fn get_verified(&self, object: &ObjectRef) -> Result<Vec<u8>, StorageError> {
        self.objects
            .lock()
            .unwrap()
            .get(object.object_id.as_str())
            .cloned()
            .ok_or(StorageError::Unavailable)
    }
    async fn presign_verified(
        &self,
        object: &ObjectRef,
        ttl: Duration,
    ) -> Result<SecretValue, StorageError> {
        assert_eq!(ttl, Duration::from_secs(90));
        if !self.head_verified(object).await? {
            return Err(StorageError::Unavailable);
        }
        self.signed.fetch_add(1, Ordering::SeqCst);
        SecretValue::new("https://private.example.invalid/opaque?X-Amz-Signature=REDACTED".into())
            .map_err(|_| StorageError::Unavailable)
    }
}

fn pin(chapter: i64, version: i64) -> ChapterVersionRef {
    ChapterVersionRef {
        chapter_id: PositiveId::new(chapter).unwrap(),
        version_id: PositiveId::new(version).unwrap(),
    }
}

#[tokio::test]
#[ignore = "requires isolated PG18 TEST_DATABASE_URL ending /tor_ci"]
async fn g_s065_only_released_exact_image_gets_a_scoped_key_and_signature() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let pool = PgPool::connect(&url).await.unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES('ci:g-image-asset','ru','Encrypted illustration','public')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Private figure','draft','paid',100) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let body = "Изображение: ![figure](private)";
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,$2) RETURNING id",
    )
    .bind(chapter)
    .bind(body)
    .fetch_one(&pool)
    .await
    .unwrap();
    let pinned = pin(chapter, version);

    let kek = Arc::new(ContentKek::from_bytes([0x37; 32]));
    let ring = Arc::new(ContentKeyring::v1_only(kek.clone()));
    let storage = PrivateFakeS3::default();
    let registry = PgStorageRegistry::new(pool.clone());
    let delivery = PgPrivateDelivery::with_keyring(
        pool.clone(),
        storage.clone(),
        ring.clone(),
        "tor-private-chapter-ci".into(),
    )
    .unwrap();

    // Signature and authenticated envelope are bound to pinned B V1.
    let image_id = ObjectId::parse("ba650000-0001-4000-8000-000000000001").unwrap();
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend_from_slice(&[0; 48]);
    let sealed =
        seal_asset_with_keyring(pinned, image_id.clone(), &png, "image/png", &ring).unwrap();
    let object = registry
        .reserve_pending_asset(&sealed, &ring, "tor-private-chapter-ci")
        .await
        .unwrap();
    let encrypted = EncryptedObjectRef {
        object: object.clone(),
        nonce: sealed.object_nonce,
        aad: sealed.aad.clone(),
        wrapped_key_id: sealed.kek_id.into(),
        codec: ContentCodec::None,
    };
    assert_eq!(encrypted.object.kind, EncryptedObjectKind::ChapterAsset);
    assert_eq!(
        delivery
            .grant_verified_object(&encrypted, Duration::from_secs(90))
            .await
            .unwrap_err(),
        DeliveryError::Unavailable,
    );
    assert_eq!(storage.signed.load(Ordering::SeqCst), 0);

    let receipt = upload_and_verify(&storage, &object, &sealed.ciphertext)
        .await
        .unwrap();
    registry.mark_ready(&receipt).await.unwrap();
    assert_eq!(
        delivery
            .grant_verified_object(&encrypted, Duration::from_secs(90))
            .await
            .unwrap_err(),
        DeliveryError::Unavailable,
    );
    // Image can be attached by B only while the exact edition is a draft.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(
        "UPDATE storage_objects SET state='attached',attached_at=now(),delete_after=NULL
         WHERE id=$1::uuid",
    )
    .bind(image_id.as_str())
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_version_assets
         (chapter_id,version_id,storage_object_id,ordinal,media_type)
         VALUES($1,$2,$3::uuid,1,'image/png')",
    )
    .bind(chapter)
    .bind(version)
    .bind(image_id.as_str())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    // Membership without a released, attached chapter is NOT a grant.
    assert_eq!(
        delivery
            .grant_verified_object(&encrypted, Duration::from_secs(90))
            .await
            .unwrap_err(),
        DeliveryError::Unavailable,
    );
    assert_eq!(storage.signed.load(Ordering::SeqCst), 0);
    let body_id = ObjectId::parse("ba650000-0002-4000-8000-000000000002").unwrap();
    let body_ciphertext = seal_chapter(pinned, body_id.clone(), body, &kek).unwrap();
    let body_ref = registry
        .reserve_pending(&body_ciphertext, &kek, "tor-private-chapter-ci")
        .await
        .unwrap();
    let receipt = upload_and_verify(&storage, &body_ref, &body_ciphertext.ciphertext)
        .await
        .unwrap();
    registry.mark_ready(&receipt).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(
        "UPDATE storage_objects SET state='attached',attached_at=now(),delete_after=NULL
         WHERE id=$1::uuid",
    )
    .bind(body_id.as_str())
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE chapter_versions SET storage_object_id=$2::uuid
         WHERE id=$1",
    )
    .bind(version)
    .bind(body_id.as_str())
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE chapters SET visibility='public' WHERE id=$1")
        .bind(chapter)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_encrypted_release_events
         (chapter_id,version_id,storage_object_id)
         VALUES($1,$2,$3::uuid)",
    )
    .bind(chapter)
    .bind(version)
    .bind(body_id.as_str())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let granted = delivery
        .grant_verified_object(&encrypted, Duration::from_secs(90))
        .await
        .unwrap();
    assert_eq!(granted.codec, "none");
    assert_eq!(storage.signed.load(Ordering::SeqCst), 1);
    let wire = serde_json::to_value(&granted).unwrap();
    let dek = URL_SAFE_NO_PAD
        .decode(wire["dek_b64u"].as_str().unwrap())
        .unwrap();
    let clear = Aes256Gcm::new_from_slice(&dek)
        .unwrap()
        .decrypt(
            Nonce::from_slice(&sealed.object_nonce),
            Payload {
                msg: sealed.ciphertext.as_slice(),
                aad: sealed.aad.as_slice(),
            },
        )
        .unwrap();
    assert_eq!(clear, png);
    assert!(!format!("{granted:?}").contains("X-Amz-Signature"));

    let mut forged = encrypted.clone();
    forged.aad[0] ^= 1;
    assert_eq!(
        delivery
            .grant_verified_object(&forged, Duration::from_secs(90))
            .await
            .unwrap_err(),
        DeliveryError::Unavailable
    );
    forged = encrypted.clone();
    forged.codec = ContentCodec::Gzip;
    assert_eq!(
        delivery
            .grant_verified_object(&forged, Duration::from_secs(90))
            .await
            .unwrap_err(),
        DeliveryError::Invalid
    );
    forged = encrypted.clone();
    forged.object.kind = EncryptedObjectKind::Chapter;
    assert_eq!(
        delivery
            .grant_verified_object(&forged, Duration::from_secs(90))
            .await
            .unwrap_err(),
        DeliveryError::Invalid
    );
    assert_eq!(storage.signed.load(Ordering::SeqCst), 1);
    pool.close().await;
}
