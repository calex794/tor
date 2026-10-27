//! S3-08 real PG18 registry. No paid chapter endpoints or remote S3 needed.
//! The live private S3 adapter is tested separately with a real MinIO instance.
#![cfg(feature = "pg")]

use async_trait::async_trait;
use sqlx::{postgres::PgPoolOptions, Row};
use std::{sync::Mutex, time::Duration};
use tor_contracts::paid_delivery::{
    ChapterVersionRef, ObjectId, ObjectRef, PositiveId, SecretValue,
};
use tor_storage::{
    domain::provider::StorageError,
    repositories::registry::{ObjectState, PgStorageRegistry, RegistryError},
    services::{
        crypto::{seal_chapter, sha256_bytes, ContentKek},
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
        if data.len() as u64 != object.size_bytes
            || sha256_bytes(data) != object.sha256_ciphertext
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
        Ok(self.ciphertext.lock().unwrap().as_deref().is_some_and(|bytes| {
            bytes.len() as u64 == object.size_bytes
        }))
    }

    async fn get_verified(&self, object: &ObjectRef) -> Result<Vec<u8>, StorageError> {
        let data = self
            .ciphertext
            .lock()
            .unwrap()
            .clone()
            .ok_or(StorageError::Unavailable)?;
        if data.len() as u64 != object.size_bytes
            || sha256_bytes(&data) != object.sha256_ciphertext
        {
            return Err(StorageError::Corrupt);
        }
        Ok(data)
    }

    async fn presign_verified(
        &self,
        _object: &ObjectRef,
        _ttl: Duration,
    ) -> Result<SecretValue, StorageError> {
        Err(StorageError::Unavailable)
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
    assert_eq!(registry.state(&id).await.unwrap(), Some(ObjectState::Pending));
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
    assert_eq!(registry.mark_ready(&receipt).await, Err(RegistryError::Conflict));
    assert_eq!(
        upload_and_verify(&storage, &object, &encrypted.ciphertext)
            .await
            .is_ok(),
        true,
        "restart after successful S3 PUT must verify existing bytes, not overwrite"
    );

    // Even the application role cannot rewrite a registered edition in PG.
    let rewrite = sqlx::query(
        "UPDATE storage_objects SET sha256_ciphertext=$2 WHERE id=$1::uuid",
    )
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
    assert_eq!(registry.state(&id).await.unwrap(), Some(ObjectState::Attached));
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
