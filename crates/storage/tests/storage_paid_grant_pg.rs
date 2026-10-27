//! G-09/S3-18: real PG18 + isolated signer prove attached-only scoped key.
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
        Arc,
    },
    time::Duration,
};
use tor_contracts::paid_delivery::{
    ChapterVersionRef, ContentCodec, DeliveryError, EncryptedObjectKind, EncryptedObjectRef,
    ObjectId, ObjectRef, PositiveId, PrivateObjectDeliveryPort, SecretValue,
};
use tor_storage::{
    domain::provider::StorageError,
    repositories::registry::PgStorageRegistry,
    services::{
        crypto::{seal_chapter, ContentKek},
        paid_delivery::PgPrivateDelivery,
        provider::PrivateObjectStore,
    },
};

#[derive(Default)]
struct FakeSignedS3 {
    calls: AtomicUsize,
    unavailable: AtomicBool,
}

#[async_trait]
impl PrivateObjectStore for FakeSignedS3 {
    async fn put_immutable(
        &self,
        _object: &ObjectRef,
        _ciphertext: &[u8],
    ) -> Result<(), StorageError> {
        Err(StorageError::Unavailable)
    }
    async fn head_verified(&self, _object: &ObjectRef) -> Result<bool, StorageError> {
        Err(StorageError::Unavailable)
    }
    async fn get_verified(&self, _object: &ObjectRef) -> Result<Vec<u8>, StorageError> {
        Err(StorageError::Unavailable)
    }
    async fn presign_verified(
        &self,
        object: &ObjectRef,
        ttl: Duration,
    ) -> Result<SecretValue, StorageError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(ttl, Duration::from_secs(90));
        assert_eq!(object.kind, EncryptedObjectKind::Chapter);
        if self.unavailable.load(Ordering::SeqCst) {
            return Err(StorageError::Unavailable);
        }
        SecretValue::new(
            "https://private-s3.example.invalid/V1?X-Amz-Signature=SECRET".to_owned(),
        )
        .map_err(|_| StorageError::Unavailable)
    }
}

#[tokio::test]
#[ignore = "requires disposable PG18 TEST_DATABASE_URL ending /tor_ci"]
async fn g_09_attached_registry_ref_only_exposes_scoped_dek_after_verified_metadata() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PG CI");
    assert!(url.ends_with("/tor_ci"), "production forbidden");
    let pool = PgPool::connect(&url).await.unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES('ci:g-private-18','ru','S3 grant test','public') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Ciphertext V1','public','paid',100) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let markdown = "Секретная V1 — А😀Б\r\n第二章";
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,$2) RETURNING id",
    )
    .bind(chapter)
    .bind(markdown)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(&pool)
        .await
        .unwrap();

    let object_id = ObjectId::parse("d318d318-0001-4001-8001-000000000018").unwrap();
    let kek = Arc::new(ContentKek::from_bytes([0x66; 32]));
    let sealed = seal_chapter(
        ChapterVersionRef {
            chapter_id: PositiveId::new(chapter).unwrap(),
            version_id: PositiveId::new(version).unwrap(),
        },
        object_id.clone(),
        markdown,
        &kek,
    )
    .unwrap();
    let registry = PgStorageRegistry::new(pool.clone());
    let object = registry
        .reserve_pending(&sealed, &kek, "tor-private-chapter-ci")
        .await
        .unwrap();
    let encrypted = EncryptedObjectRef {
        object,
        nonce: sealed.object_nonce,
        aad: sealed.aad.clone(),
        wrapped_key_id: sealed.kek_id.to_owned(),
        codec: ContentCodec::Gzip,
    };
    let s3 = Arc::new(FakeSignedS3::default());
    let grant = PgPrivateDelivery::new(pool.clone(), s3.clone(), kek.clone());

    for state in ["pending", "ready"] {
        assert_eq!(
            grant
                .grant_verified_object(&encrypted, Duration::from_secs(90))
                .await
                .unwrap_err(),
            DeliveryError::Unavailable,
            "{state} is NOT a B attached release"
        );
        if state == "pending" {
            sqlx::query("UPDATE storage_objects SET state='ready',verified_at=now() WHERE id=$1::uuid")
                .bind(object_id.as_str())
                .execute(&pool)
                .await
                .unwrap();
        }
    }
    assert_eq!(s3.calls.load(Ordering::SeqCst), 0);

    // In production only the B transaction performs ready→attached+FK.
    // Here both writes are exercised on the disposable PG fixture.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query(
        "UPDATE storage_objects SET state='attached',attached_at=now(),
           delete_after=NULL WHERE id=$1::uuid",
    )
    .bind(object_id.as_str())
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE chapter_versions SET storage_object_id=$2::uuid WHERE id=$1",
    )
    .bind(version)
    .bind(object_id.as_str())
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let granted = grant
        .grant_verified_object(&encrypted, Duration::from_secs(90))
        .await
        .unwrap();
    assert_eq!(s3.calls.load(Ordering::SeqCst), 1);
    assert_eq!(granted.object_sha256, hex::encode(encrypted.object.sha256_ciphertext));
    assert_eq!(granted.nonce_b64u, URL_SAFE_NO_PAD.encode(sealed.object_nonce));
    assert_eq!(granted.aad_b64u, URL_SAFE_NO_PAD.encode(&sealed.aad));
    let debug = format!("{granted:?}");
    assert!(!debug.contains("SECRET"), "never log signed URL or key");

    // Independently decrypt with the exact key G scopes to this object.
    let json = serde_json::to_value(&granted).unwrap();
    let key = URL_SAFE_NO_PAD
        .decode(json["dek_b64u"].as_str().unwrap())
        .unwrap();
    let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
    let zipped = cipher
        .decrypt(
            Nonce::from_slice(&sealed.object_nonce),
            Payload {
                msg: &sealed.ciphertext,
                aad: &sealed.aad,
            },
        )
        .unwrap();
    let mut plain = String::new();
    GzDecoder::new(zipped.as_slice())
        .read_to_string(&mut plain)
        .unwrap();
    assert_eq!(plain, markdown.replace("\r\n", "\n"));

    let mut wrong = encrypted.clone();
    wrong.nonce[0] ^= 1;
    assert_eq!(
        grant.grant_verified_object(&wrong, Duration::from_secs(90)).await.unwrap_err(),
        DeliveryError::Unavailable
    );
    wrong = encrypted.clone();
    wrong.aad[0] ^= 1;
    assert_eq!(
        grant.grant_verified_object(&wrong, Duration::from_secs(90)).await.unwrap_err(),
        DeliveryError::Unavailable
    );
    wrong = encrypted.clone();
    wrong.object.sha256_ciphertext[0] ^= 1;
    assert_eq!(
        grant.grant_verified_object(&wrong, Duration::from_secs(90)).await.unwrap_err(),
        DeliveryError::Unavailable
    );
    wrong = encrypted.clone();
    wrong.object.kind = EncryptedObjectKind::ChapterAsset;
    assert_eq!(
        grant.grant_verified_object(&wrong, Duration::from_secs(90)).await.unwrap_err(),
        DeliveryError::Invalid
    );
    assert_eq!(
        grant.grant_verified_object(&encrypted, Duration::from_secs(121)).await.unwrap_err(),
        DeliveryError::Invalid
    );
    assert_eq!(s3.calls.load(Ordering::SeqCst), 1);

    let wrong_key = PgPrivateDelivery::new(
        pool.clone(),
        s3.clone(),
        Arc::new(ContentKek::from_bytes([0x77; 32])),
    );
    assert_eq!(
        wrong_key
            .grant_verified_object(&encrypted, Duration::from_secs(90))
            .await
            .unwrap_err(),
        DeliveryError::Unavailable
    );
    assert_eq!(s3.calls.load(Ordering::SeqCst), 1);

    s3.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(
        grant.grant_verified_object(&encrypted, Duration::from_secs(90)).await.unwrap_err(),
        DeliveryError::Unavailable
    );
    assert_eq!(s3.calls.load(Ordering::SeqCst), 2);
    pool.close().await;
}
