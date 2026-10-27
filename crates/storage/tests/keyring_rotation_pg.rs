//! S3-34: real PostgreSQL 18 operator-only rewrap and dual-read signed G.
//! No external S3, no production roles, no operator secret printed to logs.
#![cfg(feature = "pg")]

use async_trait::async_trait;
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use tor_contracts::paid_delivery::{
    ChapterVersionRef, ContentCodec, DeliveryError, EncryptedObjectRef, ObjectId, ObjectRef,
    PositiveId, PrivateObjectDeliveryPort, SecretValue,
};
use tor_storage::{
    domain::provider::StorageError,
    repositories::{
        kek_rotation::{PgKekRotation, RotationError},
        paid_delivery::PgPrivateDelivery,
        registry::PgStorageRegistry,
    },
    services::{crypto::{seal_chapter, ContentKek, ContentKeyring}, provider::PrivateObjectStore},
};

struct FakeSigner;

#[async_trait]
impl PrivateObjectStore for FakeSigner {
    async fn put_immutable(
        &self,
        _object: &ObjectRef,
        _bytes: &[u8],
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
        _object: &ObjectRef,
        ttl: Duration,
    ) -> Result<SecretValue, StorageError> {
        assert_eq!(ttl, Duration::from_secs(90));
        SecretValue::new("https://private-s3.example.invalid/key-rotation-ci".to_owned())
            .map_err(|_| StorageError::Unavailable)
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci with tor_migrator and tor_app"]
async fn s334_rotates_only_authenticated_wrapper_and_keeps_historical_dek() {
    let app_url = std::env::var("TEST_DATABASE_URL").expect("CI app role");
    let migrator_url = std::env::var("MIGRATOR_URL").expect("CI migrator role");
    assert_eq!(
        app_url,
        "postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci"
    );
    assert_eq!(
        migrator_url,
        "postgresql://tor_migrator:tor-ci-migrate@127.0.0.1:5432/tor_ci"
    );
    assert_eq!(std::env::var("GITHUB_ACTIONS").unwrap(), "true");
    let app = PgPool::connect(&app_url).await.unwrap();
    let mig = PgPool::connect(&migrator_url).await.unwrap();

    let old = Arc::new(ContentKek::from_bytes([0x41; 32]));
    let next = Arc::new(ContentKek::from_bytes([0x42; 32]));
    let id = ObjectId::parse("f0340340-2001-4001-8001-000000000034").unwrap();
    let sealed = seal_chapter(
        ChapterVersionRef {
            chapter_id: PositiveId::new(9011).unwrap(),
            version_id: PositiveId::new(9012).unwrap(),
        },
        id.clone(),
        "Исторически купленная V1 — нельзя переписывать шифротекст",
        &old,
    )
    .unwrap();
    let registry = PgStorageRegistry::new(app.clone());
    let obj = registry
        .reserve_pending(&sealed, &old, "tor-private-chapter-ci")
        .await
        .unwrap();
    let encrypted = EncryptedObjectRef {
        object: obj,
        nonce: sealed.object_nonce,
        aad: sealed.aad.clone(),
        wrapped_key_id: "content-kek-v1".to_owned(),
        codec: ContentCodec::Gzip,
    };

    // This simulates B's authoritative attach, without a real paid purchaser.
    sqlx::query(
        "UPDATE storage_objects SET state='ready',verified_at=now()
         WHERE id=$1::uuid",
    )
    .bind(id.as_str())
    .execute(&app)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE storage_objects SET state='attached',attached_at=now(),
         delete_after=NULL WHERE id=$1::uuid",
    )
    .bind(id.as_str())
    .execute(&app)
    .await
    .unwrap();
    let before: (Vec<u8>, Vec<u8>, Vec<u8>, i64, String) = sqlx::query_as(
        "SELECT sha256_ciphertext,object_nonce,object_aad,size_bytes,object_key
         FROM storage_objects WHERE id=$1::uuid",
    )
    .bind(id.as_str())
    .fetch_one(&app)
    .await
    .unwrap();

    let v1_grant = PgPrivateDelivery::new(
        app.clone(),
        FakeSigner,
        old.clone(),
        "tor-private-chapter-ci".to_owned(),
    )
    .unwrap();
    let original = v1_grant
        .grant_verified_object(&encrypted, Duration::from_secs(90))
        .await
        .unwrap();
    let prior = serde_json::to_value(original).unwrap();

    // An app user cannot rotate data, even when it knows V2-sized bytes.
    assert!(
        sqlx::query(
            "UPDATE storage_objects
             SET encrypted_dek=$2,wrap_nonce=$3,kek_id='content-kek-v2'
             WHERE id=$1::uuid",
        )
        .bind(id.as_str())
        .bind(vec![0x61_u8; 48])
        .bind(vec![0x63_u8; 12])
        .execute(&app)
        .await
        .is_err()
    );
    assert!(
        sqlx::query_scalar::<_, bool>(
            "SELECT rewrap_attached_storage_v1_to_v2($1::uuid,$2,$3,$4,$5)",
        )
        .bind(id.as_str())
        .bind(&sealed.wrapped_dek)
        .bind(sealed.wrap_nonce.as_slice())
        .bind(vec![0x61_u8; 48])
        .bind(vec![0x63_u8; 12])
        .fetch_one(&app)
        .await
        .is_err()
    );

    let operator = PgKekRotation::new(mig.clone());
    assert!(operator.count_v1_references().await.unwrap() > 0);
    assert_eq!(
        PgKekRotation::new(app.clone())
            .rewrap_one_v1_to_v2(&id, &old, &next)
            .await,
        Err(RotationError::NotOperator)
    );
    assert_eq!(
        operator
            .rewrap_one_v1_to_v2(
                &id,
                &ContentKek::from_bytes([0x69; 32]),
                &next,
            )
            .await,
        Err(RotationError::KeyRejected)
    );
    assert!(operator
        .rewrap_one_v1_to_v2(&id, &old, &next)
        .await
        .unwrap());
    assert!(
        !operator
            .rewrap_one_v1_to_v2(&id, &old, &next)
            .await
            .unwrap(),
        "idempotent replay does not rotate a second time"
    );

    let after: (Vec<u8>, Vec<u8>, Vec<u8>, i64, String) = sqlx::query_as(
        "SELECT sha256_ciphertext,object_nonce,object_aad,size_bytes,object_key
         FROM storage_objects WHERE id=$1::uuid",
    )
    .bind(id.as_str())
    .fetch_one(&app)
    .await
    .unwrap();
    assert_eq!(before, after, "immutable S3 bytes and AAD never change");
    let new_id: String =
        sqlx::query_scalar("SELECT kek_id FROM storage_objects WHERE id=$1::uuid")
            .bind(id.as_str())
            .fetch_one(&app)
            .await
            .unwrap();
    assert_eq!(new_id, "content-kek-v2");
    assert_eq!(
        v1_grant
            .grant_verified_object(&encrypted, Duration::from_secs(90))
            .await
            .unwrap_err(),
        DeliveryError::Unavailable
    );

    let mut rotated = encrypted.clone();
    rotated.wrapped_key_id = "content-kek-v2".to_owned();
    let without_v2 = PgPrivateDelivery::new(
        app.clone(),
        FakeSigner,
        old.clone(),
        "tor-private-chapter-ci".to_owned(),
    )
    .unwrap();
    assert_eq!(
        without_v2
            .grant_verified_object(&rotated, Duration::from_secs(90))
            .await
            .unwrap_err(),
        DeliveryError::Unavailable
    );
    let restored = PgPrivateDelivery::with_keyring(
        app.clone(),
        FakeSigner,
        Arc::new(ContentKeyring::with_v2(old, next).unwrap()),
        "tor-private-chapter-ci".to_owned(),
    )
    .unwrap();
    let issued = restored
        .grant_verified_object(&rotated, Duration::from_secs(90))
        .await
        .unwrap();
    let renewed = serde_json::to_value(issued).unwrap();
    assert_eq!(prior["dek_b64u"], renewed["dek_b64u"]);
    assert_eq!(prior["object_sha256"], renewed["object_sha256"]);
    assert_eq!(prior["nonce_b64u"], renewed["nonce_b64u"]);
    assert_eq!(prior["aad_b64u"], renewed["aad_b64u"]);

    // No direct rewind or rewriting S3 metadata under a migrator role.
    assert!(sqlx::query(
        "UPDATE storage_objects SET kek_id='content-kek-v1' WHERE id=$1::uuid",
    )
    .bind(id.as_str())
    .execute(&mig)
    .await
    .is_err());
    assert!(sqlx::query(
        "UPDATE storage_objects SET sha256_ciphertext=$2 WHERE id=$1::uuid",
    )
    .bind(id.as_str())
    .bind(vec![0x44_u8; 32])
    .execute(&mig)
    .await
    .is_err());
    app.close().await;
    mig.close().await;
}
