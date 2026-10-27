//! G-09 S3-12: PG18 orphan cleanup, concurrent claims, retries and paid V1.
#![cfg(feature = "pg")]

use async_trait::async_trait;
use sqlx::{PgPool, Row};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tor_contracts::paid_delivery::{ChapterVersionRef, ObjectId, PositiveId};
use tor_storage::{
    domain::{gc::GcClaim, provider::StorageError},
    repositories::gc::PgOrphanGc,
    services::{
        crypto::{chapter_aad, sha256_bytes},
        gc::{run_one, GcError, GcOutcome, OrphanDeletePort, OrphanGcRegistry},
    },
};

const PRIVATE_BUCKET: &str = "tor-private-chapter-ci";
const A: &str = "ac120000-0000-4000-8000-000000000001";
const B: &str = "ac120000-0000-4000-8000-000000000002";
const C: &str = "ac120000-0000-4000-8000-000000000003";
const PAID_V1: &str = "ac120000-0000-4000-8000-000000000004";

async fn orphan(pool: &PgPool, id: &str, ready: bool) {
    let content = [7_u8; 64];
    sqlx::query(
        "INSERT INTO storage_objects
           (id,object_kind,bucket,object_key,state,size_bytes,
            sha256_ciphertext,plaintext_sha256,encrypted_dek,wrap_nonce,
            kek_id,object_nonce,object_aad,codec,verified_at,delete_after)
         VALUES ($1::uuid,'chapter',$2,$3,
                 CASE WHEN $4 THEN 'ready' ELSE 'pending' END,
                 64,$5,$6,$7,$8,'content-kek-v1',$9,$10,'gzip',
                 CASE WHEN $4 THEN now() ELSE NULL END,
                 now()-INTERVAL '48 hours')",
    )
    .bind(id)
    .bind(PRIVATE_BUCKET)
    .bind(format!("private/chapter-versions/{id}.enc"))
    .bind(ready)
    .bind(sha256_bytes(&content).as_slice())
    .bind([17_u8; 32].as_slice())
    .bind([18_u8; 48].as_slice())
    .bind([19_u8; 12].as_slice())
    .bind([20_u8; 12].as_slice())
    .bind([21_u8; 64].as_slice())
    .execute(pool)
    .await
    .unwrap();
}

#[derive(Default)]
struct MockStorage {
    fail_next: AtomicBool,
    delete_count: AtomicUsize,
}

#[async_trait]
impl OrphanDeletePort for MockStorage {
    async fn delete_claimed(&self, claim: &GcClaim) -> Result<(), StorageError> {
        assert_eq!(claim.bucket(), PRIVATE_BUCKET);
        assert!(claim.key().as_str().ends_with(".enc"));
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(StorageError::Unavailable);
        }
        self.delete_count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn g_09_s3_12_fenced_orphans_retry_restart_and_exhaustion() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(
        url.ends_with("/tor_ci"),
        "never touch a production database"
    );
    let pool = PgPool::connect(&url).await.unwrap();
    let gc = PgOrphanGc::new(pool.clone());

    orphan(&pool, A, false).await;
    let (left, right) = tokio::join!(gc.claim_due(), gc.claim_due());
    let mut claimed: Vec<GcClaim> = [left.unwrap(), right.unwrap()]
        .into_iter()
        .flatten()
        .collect();
    assert_eq!(claimed.len(), 1, "exclusive SKIP LOCKED claim");
    let old = claimed.pop().unwrap();
    assert_eq!(old.object_id().as_str(), A);
    assert_eq!(old.epoch(), 1);
    assert_eq!(old.attempts(), 1);
    assert!(
        gc.claim_due().await.unwrap().is_none(),
        "live lease is exclusive"
    );

    // Worker dies before the S3 delete; another process can recover after
    // lease expiration. Old worker may still delete S3, but cannot finish PG.
    sqlx::query(
        "UPDATE storage_objects SET gc_locked_until=now()-INTERVAL '1 second'
         WHERE id=$1::uuid",
    )
    .bind(A)
    .execute(&pool)
    .await
    .unwrap();
    let restarted = PgOrphanGc::new(pool.clone());
    let next = restarted.claim_due().await.unwrap().unwrap();
    assert_eq!(next.epoch(), 2);
    assert_eq!(next.attempts(), 2);
    assert_eq!(gc.finish(&old).await, Err(GcError::LostLease));
    assert_eq!(gc.retry(&old, "unavailable").await, Err(GcError::LostLease));
    assert_eq!(gc.retry(&next, "unavailable").await, Ok(()));
    let row = sqlx::query(
        "SELECT state,gc_error_code,gc_retry_after>now() AS backoff,
                gc_locked_until IS NULL AS unlocked
         FROM storage_objects WHERE id=$1::uuid",
    )
    .bind(A)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.try_get::<String, _>("state").unwrap(), "deleting");
    assert_eq!(
        row.try_get::<String, _>("gc_error_code").unwrap(),
        "unavailable"
    );
    assert!(row.try_get::<bool, _>("backoff").unwrap());
    assert!(row.try_get::<bool, _>("unlocked").unwrap());
    assert!(
        gc.claim_due().await.unwrap().is_none(),
        "backoff blocks retry"
    );
    sqlx::query(
        "UPDATE storage_objects SET gc_retry_after=now()-INTERVAL '1 second'
         WHERE id=$1::uuid",
    )
    .bind(A)
    .execute(&pool)
    .await
    .unwrap();
    let claim = gc.claim_due().await.unwrap().unwrap();
    assert_eq!(claim.epoch(), 3);
    assert_eq!(gc.finish(&claim).await, Ok(()));
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM storage_objects WHERE id=$1::uuid")
            .bind(A)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, 0);

    // A failed S3 operation records a retry, never removes its PG receipt.
    orphan(&pool, B, true).await;
    let store = MockStorage::default();
    store.fail_next.store(true, Ordering::SeqCst);
    assert_eq!(run_one(&gc, &store).await, Ok(GcOutcome::Retrying));
    assert_eq!(store.delete_count.load(Ordering::SeqCst), 0);
    assert_eq!(gc.counts().await.unwrap().1, 1);
    sqlx::query(
        "UPDATE storage_objects SET gc_retry_after=now()-INTERVAL '1 second'
         WHERE id=$1::uuid",
    )
    .bind(B)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(run_one(&gc, &store).await, Ok(GcOutcome::Deleted));
    assert_eq!(store.delete_count.load(Ordering::SeqCst), 1);
    assert_eq!(run_one(&gc, &store).await, Ok(GcOutcome::Idle));

    // Once the eighth bounded attempt fails, the object stays in PG for
    // operator inspection; no automatic infinite delete loop is possible.
    orphan(&pool, C, false).await;
    sqlx::query(
        "UPDATE storage_objects SET state='deleting', gc_attempts=7,
                gc_lease_epoch=7, gc_locked_until=now()-INTERVAL '1 second'
         WHERE id=$1::uuid",
    )
    .bind(C)
    .execute(&pool)
    .await
    .unwrap();
    store.fail_next.store(true, Ordering::SeqCst);
    assert_eq!(run_one(&gc, &store).await, Ok(GcOutcome::NeedsAttention));
    sqlx::query(
        "UPDATE storage_objects SET gc_retry_after=now()-INTERVAL '1 second'
         WHERE id=$1::uuid",
    )
    .bind(C)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(gc.counts().await.unwrap().2, 1);
    assert!(gc.claim_due().await.unwrap().is_none());
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn g_09_s3_12_paid_v1_retained_after_v2_publication() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('gc-paid-owner@example.invalid','GC paid owner','$argon2id$ci')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:gc-paid-immutable','ru','GC pinned V1','public',$1) RETURNING id",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Paid pinned chapter','public','paid',150) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let text_v1 = "Неприкосновенная V1";
    let v1: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,$2) RETURNING id",
    )
    .bind(chapter)
    .bind(text_v1)
    .fetch_one(&pool)
    .await
    .unwrap();
    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'Новая V2') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    let digest = sha256_bytes(text_v1.as_bytes());
    let id = ObjectId::parse(PAID_V1).unwrap();
    let version = ChapterVersionRef {
        chapter_id: PositiveId::new(chapter).unwrap(),
        version_id: PositiveId::new(v1).unwrap(),
    };
    let aad = chapter_aad(version, &id, &digest);
    sqlx::query(
        "INSERT INTO storage_objects
           (id,object_kind,bucket,object_key,state,size_bytes,
            sha256_ciphertext,plaintext_sha256,encrypted_dek,wrap_nonce,
            kek_id,object_nonce,object_aad,codec,verified_at,attached_at)
         VALUES ($1::uuid,'chapter',$2,$3,'attached',64,$4,$5,$6,$7,
                 'content-kek-v1',$8,$9,'gzip',now(),now())",
    )
    .bind(PAID_V1)
    .bind(PRIVATE_BUCKET)
    .bind(format!("private/chapter-versions/{PAID_V1}.enc"))
    .bind([16_u8; 32].as_slice())
    .bind(digest.as_slice())
    .bind([18_u8; 48].as_slice())
    .bind([19_u8; 12].as_slice())
    .bind([20_u8; 12].as_slice())
    .bind(aad)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_versions SET storage_object_id=$2::uuid WHERE id=$1")
        .bind(v1)
        .bind(PAID_V1)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO chapter_publications(chapter_id,version_id)
         VALUES($1,$2)",
    )
    .bind(chapter)
    .bind(v1)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(chapter)
        .bind(v2)
        .execute(&pool)
        .await
        .unwrap();

    let gc = PgOrphanGc::new(pool.clone());
    assert!(gc.claim_due().await.unwrap().is_none());
    let wrong_due = sqlx::query(
        "UPDATE storage_objects SET delete_after=now()-INTERVAL '1 hour'
         WHERE id=$1::uuid",
    )
    .bind(PAID_V1)
    .execute(&pool)
    .await;
    assert!(
        wrong_due.is_err(),
        "purchased V1 must never be eligible for GC"
    );
    let gone = sqlx::query("DELETE FROM storage_objects WHERE id=$1::uuid")
        .bind(PAID_V1)
        .execute(&pool)
        .await;
    assert!(gone.is_err(), "attached historical object is never deleted");
    let unchanged: String =
        sqlx::query_scalar("SELECT storage_object_id::text FROM chapter_versions WHERE id=$1")
            .bind(v1)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(unchanged, PAID_V1);
    pool.close().await;
}
