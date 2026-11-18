//! FND-00c: owner-paid S3 source is available only after live B/G checks.
//! Run solely against disposable PG18 /tor_ci. FakeS3 has no network access.
#![cfg(feature = "pg")]

use async_trait::async_trait;
use sqlx::PgPool;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tor_contracts::paid_delivery::{
    ChapterVersionRef, ObjectId, ObjectRef, PositiveId, SecretValue,
};
use tor_storage::{
    domain::provider::StorageError,
    repositories::registry::PgStorageRegistry,
    services::{
        crypto::{seal_chapter_with_keyring, ContentKek, ContentKeyring},
        provider::{upload_and_verify, PrivateObjectStore},
    },
};

#[derive(Default)]
struct FakeS3 {
    bytes: Mutex<Option<Vec<u8>>>,
    gets: AtomicUsize,
}

impl FakeS3 {
    fn corrupt(&self) {
        self.bytes.lock().unwrap().as_mut().unwrap()[0] ^= 1;
    }

    fn reads(&self) -> usize {
        self.gets.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl PrivateObjectStore for FakeS3 {
    async fn put_immutable(&self, _object: &ObjectRef, data: &[u8]) -> Result<(), StorageError> {
        let mut slot = self.bytes.lock().unwrap();
        if slot.is_some() {
            return Err(StorageError::Conflict);
        }
        *slot = Some(data.to_vec());
        Ok(())
    }
    async fn head_verified(&self, _object: &ObjectRef) -> Result<bool, StorageError> {
        Ok(self.bytes.lock().unwrap().is_some())
    }
    async fn get_verified(&self, _object: &ObjectRef) -> Result<Vec<u8>, StorageError> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        self.bytes
            .lock()
            .unwrap()
            .clone()
            .ok_or(StorageError::Unavailable)
    }
    async fn presign_verified(
        &self,
        _object: &ObjectRef,
        _ttl: Duration,
    ) -> Result<SecretValue, StorageError> {
        Err(StorageError::Unavailable)
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 ending in /tor_ci"]
async fn fnd00_author_source_checks_owner_release_and_actual_ciphertext() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "production database is forbidden");
    let pool = PgPool::connect(&url).await.unwrap();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('fnd00-author-source@example.invalid','S3 paid owner','$argon2id$fixture')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let stranger: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('fnd00-author-stranger@example.invalid','Other reader','$argon2id$fixture')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
         VALUES('ci:fnd00-owned-s3','ru','Protected paid book','public',$1,'basic')
         RETURNING id",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount,edit_revision)
         VALUES($1,1,'Paid bodyless editor','draft','paid',100,1) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let source = "# S3-only платный исходник V1";
    let plaintext_hash = tor_storage::services::crypto::sha256_bytes(source.as_bytes());
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown,content_sha256)
         VALUES($1,1,NULL,$2) RETURNING id",
    )
    .bind(chapter)
    .bind(plaintext_hash.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    let reference = ChapterVersionRef {
        chapter_id: PositiveId::new(chapter).unwrap(),
        version_id: PositiveId::new(version).unwrap(),
    };
    let ring = ContentKeyring::v1_only(Arc::new(ContentKek::from_bytes([31; 32])));
    let object_id = ObjectId::parse("c0a00000-0001-4000-8000-000000000041").unwrap();
    let sealed = seal_chapter_with_keyring(reference, object_id.clone(), source, &ring).unwrap();
    let registry = PgStorageRegistry::new(pool.clone());
    let store = FakeS3::default();
    let bucket = "tor-private-chapter-ci";

    // No release, and a stranger cannot cause even one ciphertext GET.
    assert!(registry
        .owned_current_paid_markdown(stranger, book, chapter, bucket, &store, &ring)
        .await
        .unwrap()
        .is_none());
    assert!(registry
        .owned_current_paid_markdown(owner, book, chapter, bucket, &store, &ring)
        .await
        .unwrap()
        .is_none());
    assert_eq!(store.reads(), 0);

    let object = registry
        .reserve_pending_with_keyring(&sealed, &ring, bucket)
        .await
        .unwrap();
    let receipt = upload_and_verify(&store, &object, &sealed.ciphertext)
        .await
        .unwrap();
    registry.mark_ready(&receipt).await.unwrap();
    sqlx::query(
        "UPDATE storage_objects SET state='attached',attached_at=now(),
          delete_after=NULL WHERE id=$1::uuid",
    )
    .bind(object_id.as_str())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_versions SET storage_object_id=$2::uuid WHERE id=$1")
        .bind(version)
        .bind(object_id.as_str())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_reader_manifests(
            chapter_id,version_id,storage_object_id,parser_schema,
            content_sha256,block_utf16_lengths,
            last_readable_block_index,last_readable_char_offset
         ) VALUES($1,$2,$3::uuid,1,$4,$5,1,$6)",
    )
    .bind(chapter)
    .bind(version)
    .bind(object_id.as_str())
    .bind(plaintext_hash.as_slice())
    .bind(vec![source.encode_utf16().count() as i32])
    .bind(source.encode_utf16().count() as i32)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_first_release_events(chapter_id,version_id,creator_user_id)
         VALUES($1,$2,$3)",
    )
    .bind(chapter)
    .bind(version)
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapters SET visibility='public' WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_encrypted_release_events(
           chapter_id,version_id,storage_object_id
         ) VALUES($1,$2,$3::uuid)",
    )
    .bind(chapter)
    .bind(version)
    .bind(object_id.as_str())
    .execute(&pool)
    .await
    .unwrap();

    // Only the active owner gets text; rejected selectors must not reach G/S3.
    let original_gets = store.reads();
    assert!(registry
        .owned_current_paid_markdown(stranger, book, chapter, bucket, &store, &ring)
        .await
        .unwrap()
        .is_none());
    assert!(registry
        .owned_current_paid_markdown(owner, book + 100, chapter, bucket, &store, &ring)
        .await
        .unwrap()
        .is_none());
    assert!(registry
        .owned_current_paid_markdown(owner, book, chapter, "other-private-ci", &store, &ring)
        .await
        .unwrap()
        .is_none());
    assert_eq!(store.reads(), original_gets);
    let body = registry
        .owned_current_paid_markdown(owner, book, chapter, bucket, &store, &ring)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(body.as_str(), source);

    // B hold must revoke owner source before an S3 request.
    sqlx::query("UPDATE chapters SET admin_hold=TRUE WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    let before_denial = store.reads();
    assert!(registry
        .owned_current_paid_markdown(owner, book, chapter, bucket, &store, &ring)
        .await
        .unwrap()
        .is_none());
    assert_eq!(store.reads(), before_denial);
    sqlx::query("UPDATE chapters SET admin_hold=FALSE WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();

    store.corrupt();
    assert!(
        registry
            .owned_current_paid_markdown(owner, book, chapter, bucket, &store, &ring)
            .await
            .is_err(),
        "G must not expose damaged ciphertext even with valid ownership"
    );
    pool.close().await;
}
