//! S3-08 real PG18 registry. No paid chapter endpoints or remote S3 needed.
//! The live private S3 adapter is tested separately with a real MinIO instance.
#![cfg(feature = "pg")]

use async_trait::async_trait;
use sqlx::{postgres::PgPoolOptions, Row};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tor_contracts::paid_delivery::{
    ChapterVersionRef, ContentCodec, EncryptedObjectRef, ObjectId, ObjectRef, PositiveId,
    PrivateObjectDeliveryPort, SecretValue,
};
use tor_storage::{
    domain::provider::StorageError,
    repositories::{
        paid_delivery::PgPrivateDelivery,
        registry::{ObjectState, PgStorageRegistry, RegistryError},
    },
    services::{
        crypto::{
            seal_chapter, seal_chapter_with_keyring, sha256_bytes, ContentKek, ContentKeyring,
        },
        provider::{upload_and_verify, PrivateObjectStore},
    },
};

#[derive(Default)]
struct FakePrivateS3 {
    ciphertext: Mutex<Option<Vec<u8>>>,
}

impl FakePrivateS3 {
    fn corrupt_for_test(&self) {
        let mut stored = self.ciphertext.lock().unwrap();
        let bytes = stored.as_mut().unwrap();
        bytes[0] ^= 1;
    }
}

#[async_trait]
impl PrivateObjectStore for FakePrivateS3 {
    async fn put_immutable(&self, object: &ObjectRef, data: &[u8]) -> Result<(), StorageError> {
        if data.len() as u64 != object.size_bytes || sha256_bytes(data) != object.sha256_ciphertext
        {
            return Err(StorageError::Invalid);
        }
        let mut slot = self.ciphertext.lock().unwrap();
        if slot.is_some() {
            return Err(StorageError::Conflict);
        }
        *slot = Some(data.to_vec());
        Ok(())
    }

    async fn head_verified(&self, object: &ObjectRef) -> Result<bool, StorageError> {
        Ok(self
            .ciphertext
            .lock()
            .unwrap()
            .as_deref()
            .is_some_and(|bytes| bytes.len() as u64 == object.size_bytes))
    }

    async fn get_verified(&self, object: &ObjectRef) -> Result<Vec<u8>, StorageError> {
        let data = self
            .ciphertext
            .lock()
            .unwrap()
            .clone()
            .ok_or(StorageError::Unavailable)?;
        if data.len() as u64 != object.size_bytes || sha256_bytes(&data) != object.sha256_ciphertext
        {
            return Err(StorageError::Corrupt);
        }
        Ok(data)
    }

    async fn presign_verified(
        &self,
        object: &ObjectRef,
        ttl: Duration,
    ) -> Result<SecretValue, StorageError> {
        if ttl != Duration::from_secs(90) || !self.head_verified(object).await? {
            return Err(StorageError::Unavailable);
        }
        SecretValue::new("https://private-s3.example.invalid/s334-signed".to_owned())
            .map_err(|_| StorageError::Unavailable)
    }
}

fn test_version() -> ChapterVersionRef {
    ChapterVersionRef {
        chapter_id: PositiveId::new(101).unwrap(),
        version_id: PositiveId::new(201).unwrap(),
    }
}

fn test_object_id() -> ObjectId {
    ObjectId::parse("c009c009-0001-4001-8001-000000000008").unwrap()
}

#[tokio::test]
#[ignore = "requires the isolated tor_ci PG18 runtime database"]
async fn g_09_s3_08_pending_verified_ready_immutability_and_retention() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG CI fixture");
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await
        .unwrap();
    let registry = PgStorageRegistry::new(pool.clone());
    let kek = ContentKek::from_bytes([77; 32]);
    let plain_marker = "C09-PRIVATE-PAID-MARKDOWN-DO-NOT-STORE";
    let mut encrypted = seal_chapter(test_version(), test_object_id(), plain_marker, &kek).unwrap();
    let id = encrypted.object_id.clone();
    assert_eq!(registry.state(&id).await.unwrap(), None);

    assert_eq!(
        registry
            .reserve_pending(&encrypted, &kek, "public-assets")
            .await,
        Err(RegistryError::Invalid)
    );
    let object = registry
        .reserve_pending(&encrypted, &kek, "tor-private-chapter-ci")
        .await
        .unwrap();
    assert_eq!(
        registry.state(&id).await.unwrap(),
        Some(ObjectState::Pending)
    );
    assert_eq!(
        registry
            .reserve_pending(&encrypted, &kek, "tor-private-chapter-ci")
            .await,
        Err(RegistryError::Conflict)
    );

    // PostgreSQL contains only wrapped DEK and ciphertext receipt metadata.
    let row = sqlx::query(
        "SELECT object_key,codec,state,octet_length(encrypted_dek)::int AS dek_len,
                octet_length(object_nonce)::int AS nonce_len,
                octet_length(wrap_nonce)::int AS wrap_len,delete_after IS NOT NULL AS gc
         FROM storage_objects WHERE id=$1::uuid",
    )
    .bind(id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    let key: String = row.try_get("object_key").unwrap();
    assert_eq!(
        key,
        "private/chapter-versions/c009c009-0001-4001-8001-000000000008.enc"
    );
    assert_eq!(row.try_get::<String, _>("state").unwrap(), "pending");
    assert_eq!(row.try_get::<String, _>("codec").unwrap(), "gzip");
    assert_eq!(row.try_get::<i32, _>("dek_len").unwrap(), 48);
    assert_eq!(row.try_get::<i32, _>("nonce_len").unwrap(), 12);
    assert_eq!(row.try_get::<i32, _>("wrap_len").unwrap(), 12);
    assert!(row.try_get::<bool, _>("gc").unwrap());

    // Invalid wrapped DEK cannot create a new registry row.
    let original_wrap = encrypted.wrapped_dek.clone();
    encrypted.wrapped_dek[0] ^= 1;
    assert_eq!(
        registry
            .reserve_pending(&encrypted, &kek, "tor-private-chapter-ci")
            .await,
        Err(RegistryError::Invalid)
    );
    encrypted.wrapped_dek = original_wrap;

    let storage = FakePrivateS3::default();
    let receipt = upload_and_verify(&storage, &object, &encrypted.ciphertext)
        .await
        .unwrap();
    assert_eq!(registry.mark_ready(&receipt).await, Ok(()));
    assert_eq!(registry.state(&id).await.unwrap(), Some(ObjectState::Ready));
    assert_eq!(
        registry.mark_ready(&receipt).await,
        Err(RegistryError::Conflict)
    );
    assert_eq!(
        upload_and_verify(&storage, &object, &encrypted.ciphertext)
            .await
            .is_ok(),
        true,
        "restart after successful S3 PUT must verify existing bytes, not overwrite"
    );

    // Even the application role cannot rewrite a registered edition in PG.
    let rewrite = sqlx::query("UPDATE storage_objects SET sha256_ciphertext=$2 WHERE id=$1::uuid")
        .bind(id.as_str())
        .bind(vec![123_u8; 32])
        .execute(&pool)
        .await;
    assert!(rewrite.is_err(), "ciphertext hash must be immutable");
    let rewind = sqlx::query(
        "UPDATE storage_objects SET state='pending',verified_at=NULL
         WHERE id=$1::uuid",
    )
    .bind(id.as_str())
    .execute(&pool)
    .await;
    assert!(rewind.is_err(), "ready cannot return to pending");

    storage.corrupt_for_test();
    assert!(matches!(
        upload_and_verify(&storage, &object, &encrypted.ciphertext).await,
        Err(StorageError::Corrupt)
    ));
    assert_eq!(registry.state(&id).await.unwrap(), Some(ObjectState::Ready));

    // B association is a separate owner-controlled future transaction. This
    // direct SQL simulates the attached state solely to test DB retention.
    sqlx::query(
        "UPDATE storage_objects SET state='attached',attached_at=now(),
            delete_after=NULL WHERE id=$1::uuid",
    )
    .bind(id.as_str())
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        registry.state(&id).await.unwrap(),
        Some(ObjectState::Attached)
    );
    assert!(
        sqlx::query("DELETE FROM storage_objects WHERE id=$1::uuid")
            .bind(id.as_str())
            .execute(&pool)
            .await
            .is_err(),
        "attached paid edition must be retained"
    );
    assert!(
        sqlx::query(
            "UPDATE storage_objects SET delete_after=now()+INTERVAL '1 hour'
             WHERE id=$1::uuid"
        )
        .bind(id.as_str())
        .execute(&pool)
        .await
        .is_err(),
        "attached edition cannot be reaped by GC"
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 TEST_DATABASE_URL ending /tor_ci"]
async fn s334_new_v2_envelope_is_registered_and_read_alongside_old_v1() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG18 CI fixture");
    assert!(url.ends_with("/tor_ci"), "never operate on production");
    let pool = PgPoolOptions::new()
        .max_connections(3)
        .connect(&url)
        .await
        .unwrap();
    let registry = PgStorageRegistry::new(pool.clone());
    let old = Arc::new(ContentKek::from_bytes([0x81; 32]));
    let next = Arc::new(ContentKek::from_bytes([0x82; 32]));
    let old_keyring = ContentKeyring::with_v2(old.clone(), next.clone()).unwrap();
    let new_keyring = ContentKeyring::with_v2(old.clone(), next)
        .unwrap()
        .with_active_id("content-kek-v2")
        .unwrap();
    let chapter_id = PositiveId::new(340).unwrap();
    let v1 = ChapterVersionRef {
        chapter_id,
        version_id: PositiveId::new(341).unwrap(),
    };
    let v2 = ChapterVersionRef {
        chapter_id,
        version_id: PositiveId::new(342).unwrap(),
    };
    let old_id = ObjectId::parse("c009c009-0001-4001-8001-000000000341").unwrap();
    let new_id = ObjectId::parse("c009c009-0001-4001-8001-000000000342").unwrap();
    let old_sealed = seal_chapter_with_keyring(
        v1,
        old_id.clone(),
        "Купленная историческая V1",
        &old_keyring,
    )
    .unwrap();
    let new_sealed =
        seal_chapter_with_keyring(v2, new_id.clone(), "Новое платное издание V2", &new_keyring)
            .unwrap();
    assert_eq!(old_sealed.kek_id, "content-kek-v1");
    assert_eq!(new_sealed.kek_id, "content-kek-v2");
    assert_eq!(
        registry
            .reserve_pending_with_keyring(&new_sealed, &old_keyring, "tor-private-chapter-ci")
            .await,
        Err(RegistryError::Invalid),
        "V1-active writer may not insert a V2 record"
    );
    assert_eq!(
        registry
            .reserve_pending_with_keyring(&old_sealed, &new_keyring, "tor-private-chapter-ci")
            .await,
        Err(RegistryError::Invalid),
        "V2-active writer may not silently fall back to V1"
    );
    assert_eq!(
        registry
            .reserve_pending(&new_sealed, &old, "tor-private-chapter-ci")
            .await,
        Err(RegistryError::Invalid),
        "legacy V1 method never admits V2"
    );

    let old_object = registry
        .reserve_pending_with_keyring(&old_sealed, &old_keyring, "tor-private-chapter-ci")
        .await
        .unwrap();
    let snapshot: (String, Vec<u8>, Vec<u8>) = sqlx::query_as(
        "SELECT kek_id,encrypted_dek,sha256_ciphertext
         FROM storage_objects WHERE id=$1::uuid",
    )
    .bind(old_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    let new_object = registry
        .reserve_pending_with_keyring(&new_sealed, &new_keyring, "tor-private-chapter-ci")
        .await
        .unwrap();
    let old_store = FakePrivateS3::default();
    let new_store = FakePrivateS3::default();
    let old_receipt = upload_and_verify(&old_store, &old_object, &old_sealed.ciphertext)
        .await
        .unwrap();
    let new_receipt = upload_and_verify(&new_store, &new_object, &new_sealed.ciphertext)
        .await
        .unwrap();
    registry.mark_ready(&old_receipt).await.unwrap();
    registry.mark_ready(&new_receipt).await.unwrap();

    // B normally performs the owner/purchase authorization and attachment;
    // this storage-level test switches state only to exercise the scoped G
    // signer. Full author and purchase policy belongs to B and D tests.
    for object_id in [&old_id, &new_id] {
        sqlx::query(
            "UPDATE storage_objects SET state='attached',attached_at=now(),
             delete_after=NULL WHERE id=$1::uuid",
        )
        .bind(object_id.as_str())
        .execute(&pool)
        .await
        .unwrap();
    }
    let after: (String, Vec<u8>, Vec<u8>) = sqlx::query_as(
        "SELECT kek_id,encrypted_dek,sha256_ciphertext
         FROM storage_objects WHERE id=$1::uuid",
    )
    .bind(old_id.as_str())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(snapshot, after, "V1 encrypted material never changes");
    let persisted_id: String =
        sqlx::query_scalar("SELECT kek_id FROM storage_objects WHERE id=$1::uuid")
            .bind(new_id.as_str())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(persisted_id, "content-kek-v2");

    let old_encrypted = EncryptedObjectRef {
        object: old_object,
        nonce: old_sealed.object_nonce,
        aad: old_sealed.aad.clone(),
        wrapped_key_id: old_sealed.kek_id.to_owned(),
        codec: ContentCodec::Gzip,
    };
    let new_encrypted = EncryptedObjectRef {
        object: new_object,
        nonce: new_sealed.object_nonce,
        aad: new_sealed.aad.clone(),
        wrapped_key_id: new_sealed.kek_id.to_owned(),
        codec: ContentCodec::Gzip,
    };
    let old_signer = PgPrivateDelivery::with_keyring(
        pool.clone(),
        old_store,
        Arc::new(old_keyring),
        "tor-private-chapter-ci".to_owned(),
    )
    .unwrap();
    let new_signer = PgPrivateDelivery::with_keyring(
        pool.clone(),
        new_store,
        Arc::new(new_keyring),
        "tor-private-chapter-ci".to_owned(),
    )
    .unwrap();
    let old_grant = old_signer
        .grant_verified_object(&old_encrypted, Duration::from_secs(90))
        .await
        .unwrap();
    let new_grant = new_signer
        .grant_verified_object(&new_encrypted, Duration::from_secs(90))
        .await
        .unwrap();
    let old_grant = serde_json::to_value(old_grant).unwrap();
    let new_grant = serde_json::to_value(new_grant).unwrap();
    assert_ne!(old_grant["dek_b64u"], new_grant["dek_b64u"]);
    assert_eq!(
        old_grant["object_sha256"],
        hex::encode(old_sealed.ciphertext_sha256)
    );
    assert_eq!(
        new_grant["object_sha256"],
        hex::encode(new_sealed.ciphertext_sha256)
    );
    pool.close().await;
}
