//! S3-RECON-01: durable read-only inventory, 2501 keys, restart and fencing.
#![cfg(feature = "pg")]

use async_trait::async_trait;
use sqlx::PgPool;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, SystemTime},
};
use tor_storage::{
    domain::provider::StorageError,
    repositories::reconciliation::{InventoryError, PgPrivateInventory},
    services::reconciliation::{
        ListedPrivateObject, PrivateInventoryLister, PrivateInventoryPage, PrivateScope,
    },
};

const BUCKET: &str = "tor-private-recon-ci";

#[derive(Default)]
struct MockList {
    fail_next: AtomicBool,
}

#[async_trait]
impl PrivateInventoryLister for MockList {
    fn private_bucket(&self) -> &str {
        BUCKET
    }

    async fn list_private_page(
        &self,
        scope: PrivateScope,
        continuation: Option<&str>,
    ) -> Result<PrivateInventoryPage, StorageError> {
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(StorageError::Unavailable);
        }
        let start = continuation
            .unwrap_or("0")
            .parse::<usize>()
            .map_err(|_| StorageError::Invalid)?;
        let end = (start + 1000).min(2501);
        let objects = (start..end)
            .map(|n| ListedPrivateObject {
                key: format!(
                    "{}{:08x}-dcb1-4c1a-a000-{:012x}.enc",
                    scope.prefix(),
                    n + 1,
                    n + 1
                ),
                size_bytes: Some(64),
                modified_at: Some(SystemTime::now() - Duration::from_secs(172800)),
            })
            .collect();
        Ok(PrivateInventoryPage {
            objects,
            continuation: if end == 2501 {
                None
            } else {
                Some(end.to_string())
            },
        })
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL /tor_ci"]
async fn g_09_s3_recon_2501_private_keys_restart_and_no_delete() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG");
    assert!(url.ends_with("/tor_ci"), "never scan against live database");
    let pool = PgPool::connect(&url).await.unwrap();
    let mock = MockList::default();
    let inventory = PgPrivateInventory::new(pool.clone());

    let first = inventory
        .scan_page(&mock, PrivateScope::Chapter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.scanned, 1000);
    assert_eq!(first.unregistered_candidates, 1000);
    assert!(!first.completed);

    // Another process reconstructs its repository and uses the persisted token.
    let restarted = PgPrivateInventory::new(pool.clone());
    let second = restarted
        .scan_page(&mock, PrivateScope::Chapter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(second.scanned, 2000);
    let third = inventory
        .scan_page(&mock, PrivateScope::Chapter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(third.scanned, 2501);
    assert_eq!(third.unregistered_candidates, 2501);
    assert!(third.completed);
    assert!(
        inventory
            .scan_page(&mock, PrivateScope::Chapter)
            .await
            .unwrap()
            .is_none(),
        "a completed cycle cannot generate endless duplicate observations"
    );
    let unique: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM g09_private_inventory_findings
          WHERE bucket=$1 AND scope='chapter' AND cycle=1",
    )
    .bind(BUCKET)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unique, 2501);

    let state: (Option<String>, bool) = sqlx::query_as(
        "SELECT continuation,completed FROM g09_private_inventory_scans
          WHERE bucket=$1 AND scope='chapter'",
    )
    .bind(BUCKET)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state, (None, true));

    // LIST failure leaves the checkpoint unchanged; neither worker can delete.
    mock.fail_next.store(true, Ordering::SeqCst);
    assert_eq!(
        inventory
            .scan_page(&mock, PrivateScope::ChapterAsset)
            .await
            .unwrap_err(),
        InventoryError::Unavailable
    );
    let after_failure = inventory
        .progress(BUCKET, PrivateScope::ChapterAsset)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after_failure.scanned, 0);

    // A live lease blocks a concurrent scanner; expiration permits recovery.
    sqlx::query(
        "UPDATE g09_private_inventory_scans
            SET leased_until=now()+INTERVAL '5 minutes'
          WHERE bucket=$1 AND scope='chapter_asset'",
    )
    .bind(BUCKET)
    .execute(&pool)
    .await
    .unwrap();
    assert!(
        inventory
            .scan_page(&mock, PrivateScope::ChapterAsset)
            .await
            .unwrap()
            .is_none(),
        "second scanner cannot steal a current lease"
    );
    sqlx::query(
        "UPDATE g09_private_inventory_scans
            SET leased_until=now()-INTERVAL '1 second'
          WHERE bucket=$1 AND scope='chapter_asset'",
    )
    .bind(BUCKET)
    .execute(&pool)
    .await
    .unwrap();
    let recovered = restarted
        .scan_page(&mock, PrivateScope::ChapterAsset)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovered.scanned, 1000);
    assert!(!recovered.completed);

    let registry_objects: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM storage_objects WHERE bucket=$1")
            .bind(BUCKET)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        registry_objects, 0,
        "inventory never writes the live registry"
    );
    assert!(
        inventory.begin_next_cycle(BUCKET, PrivateScope::Chapter)
            .await.unwrap(),
        "only an explicitly completed scan can be restarted"
    );
    assert!(
        !inventory.begin_next_cycle(BUCKET, PrivateScope::Chapter)
            .await.unwrap(),
        "an active new cycle cannot be overwritten"
    );
    let resumed = inventory.progress(BUCKET, PrivateScope::Chapter)
        .await.unwrap().unwrap();
    assert_eq!(resumed.scanned, 0);
    assert!(!resumed.completed);
    let previous: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM g09_private_inventory_findings
         WHERE bucket=$1 AND scope='chapter' AND cycle=1",
    )
    .bind(BUCKET)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(previous, 2501, "old findings must survive next cycle");
    pool.close().await;
}

const REVERSE_BUCKET: &str = "tor-private-reverse-audit-ci";
const REVERSE_V1: &str = "dd090000-0000-4000-8000-000000000001";

#[derive(Default)]
struct FakeHead {
    fail_remaining: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl tor_storage::services::reconciliation::PrivateRegisteredHead for FakeHead {
    fn private_bucket(&self) -> &str {
        REVERSE_BUCKET
    }

    async fn head_registered(
        &self,
        object: &tor_contracts::paid_delivery::ObjectRef,
    ) -> Result<tor_storage::services::reconciliation::RegisteredHeadResult, StorageError> {
        use tor_storage::services::reconciliation::RegisteredHeadResult;
        if self
            .fail_remaining
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
            .is_ok()
        {
            return Err(StorageError::Unavailable);
        }
        let n =
            u32::from_str_radix(object.object_id.as_str().rsplit('-').next().unwrap(), 16).unwrap();
        Ok(match n {
            1 | 3 | 5 => RegisteredHeadResult::Missing,
            2 => RegisteredHeadResult::MetadataMismatch,
            _ => RegisteredHeadResult::Present,
        })
    }
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 /tor_ci"]
async fn g_09_reverse_registered_2501_resume_reference_and_unknown_fail_closed() {
    use sha2::{Digest, Sha256};
    use std::sync::atomic::AtomicUsize;
    use tor_contracts::paid_delivery::{ChapterVersionRef, ObjectId, PositiveId};
    use tor_storage::repositories::reconciliation_reverse::PgRegisteredAudit;
    use tor_storage::services::crypto::{chapter_aad, sha256_bytes};

    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.ends_with("/tor_ci"), "never access production DB");
    let pool = PgPool::connect(&url).await.unwrap();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('g09-reverse-owner@example.invalid','G09 audit owner','$argon2id$ci')
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:g09-reverse-audit','ru','G09 reverse immutable V1','public',$1)
         RETURNING id",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'G09 paid V1','public','paid',150) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Pinned V1 for reconciliation') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    let body = "Pinned V1 for reconciliation";
    let content_digest = sha256_bytes(body.as_bytes());
    let aad = chapter_aad(
        ChapterVersionRef {
            chapter_id: PositiveId::new(chapter).unwrap(),
            version_id: PositiveId::new(version).unwrap(),
        },
        &ObjectId::parse(REVERSE_V1).unwrap(),
        &content_digest,
    );
    // Real foreign keys and B attachment guards: 1 attached paid V1,
    // 2500 pending objects. A fresh pending PUT is never a missing alert.
    sqlx::query(
        "WITH ids AS (
             SELECT n, ('dd090000-0000-4000-8000-' ||
                        lpad(to_hex(n),12,'0'))::uuid AS oid
               FROM generate_series(1,2501) AS x(n)
         )
         INSERT INTO storage_objects
            (id,object_kind,bucket,object_key,state,size_bytes,
             sha256_ciphertext,plaintext_sha256,encrypted_dek,wrap_nonce,
             kek_id,object_nonce,object_aad,codec,verified_at,attached_at,created_at)
         SELECT oid,'chapter',$1,
                'private/chapter-versions/' || oid::text || '.enc',
                CASE WHEN n=1 THEN 'attached' ELSE 'pending' END,
                64,decode(repeat('0a',32),'hex'),$3::bytea,
                decode(repeat('12',48),'hex'),decode(repeat('13',12),'hex'),
                'content-kek-v1',decode(repeat('14',12),'hex'),
                CASE WHEN n=1 THEN $2::bytea ELSE decode(repeat('15',64),'hex') END,
                'gzip',CASE WHEN n=1 THEN now() ELSE NULL END,
                CASE WHEN n=1 THEN now() ELSE NULL END,
                CASE WHEN n=3 THEN now() ELSE now()-interval '48 hours' END
           FROM ids",
    )
    .bind(REVERSE_BUCKET)
    .bind(aad)
    .bind(content_digest.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_versions SET storage_object_id=$2::uuid WHERE id=$1")
        .bind(version)
        .bind(REVERSE_V1)
        .execute(&pool)
        .await
        .unwrap();

    let audit = PgRegisteredAudit::new(pool.clone());
    let head = FakeHead {
        fail_remaining: AtomicUsize::new(1),
    };
    assert_eq!(
        audit
            .scan_page(&head, PrivateScope::Chapter)
            .await
            .unwrap_err(),
        InventoryError::Unavailable,
    );
    let failed = audit
        .progress(REVERSE_BUCKET, PrivateScope::Chapter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(failed.scanned, 0, "outage cannot advance the cursor");
    assert_eq!(failed.unknown_attempts, 1);
    assert_eq!(failed.retries, 1);
    assert_eq!(failed.expected, 2501);
    assert_eq!(failed.missing, 0, "S3 outage is not a missing file");

    sqlx::query(
        "UPDATE g09_registered_object_scans
            SET leased_until=clock_timestamp()+interval '1 hour'
          WHERE bucket=$1 AND scope='chapter'",
    )
    .bind(REVERSE_BUCKET)
    .execute(&pool)
    .await
    .unwrap();
    assert!(audit
        .scan_page(&head, PrivateScope::Chapter)
        .await
        .unwrap()
        .is_none());
    sqlx::query(
        "UPDATE g09_registered_object_scans
            SET leased_until=clock_timestamp()-interval '1 second'
          WHERE bucket=$1 AND scope='chapter'",
    )
    .bind(REVERSE_BUCKET)
    .execute(&pool)
    .await
    .unwrap();

    let restarted = PgRegisteredAudit::new(pool.clone());
    let mut rounds = 0_usize;
    while let Some(progress) = restarted
        .scan_page(&head, PrivateScope::Chapter)
        .await
        .unwrap()
    {
        rounds += 1;
        assert!(
            rounds <= 21,
            "keyset cursor must reach the final registered object"
        );
        if progress.completed {
            break;
        }
    }
    let done = audit
        .progress(REVERSE_BUCKET, PrivateScope::Chapter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(done.scanned, 2501);
    assert_eq!(done.expected, 2501);
    assert_eq!(done.has_ref, 1, "paid pinned V1 is a live reference");
    assert_eq!(done.missing, 2);
    assert_eq!(done.referenced_missing, 1);
    assert_eq!(done.mismatch, 1);
    assert_eq!(done.recently_pending, 1);
    assert_eq!(done.healthy, 2497);
    assert!(done.completed);
    assert!(audit
        .scan_page(&head, PrivateScope::Chapter)
        .await
        .unwrap()
        .is_none());
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM g09_registered_object_findings
          WHERE bucket=$1 AND scope='chapter' AND cycle=1",
    )
    .bind(REVERSE_BUCKET)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 2501, "checkpoint retries cannot duplicate findings");
    let key_hash = Sha256::digest(format!("private/chapter-versions/{REVERSE_V1}.enc").as_bytes());
    let status: (String, bool) = sqlx::query_as(
        "SELECT status,has_ref FROM g09_registered_object_findings
          WHERE bucket=$1 AND scope='chapter' AND cycle=1 AND key_sha256=$2",
    )
    .bind(REVERSE_BUCKET)
    .bind(key_hash.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, ("referenced_but_missing".into(), true));
    let pinned: String =
        sqlx::query_scalar("SELECT storage_object_id::text FROM chapter_versions WHERE id=$1")
            .bind(version)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(pinned, REVERSE_V1, "audit must never change pinned V1");
    assert_eq!(
        audit
            .begin_next_cycle(REVERSE_BUCKET, PrivateScope::Chapter)
            .await
            .unwrap(),
        true
    );
    let next = audit
        .progress(REVERSE_BUCKET, PrivateScope::Chapter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((next.cycle, next.scanned, next.expected), (2, 0, 2501));

    // Eight provider outages stop repeated scanning without deleting anything.
    head.fail_remaining.store(8, Ordering::SeqCst);
    for _ in 0..8 {
        assert_eq!(
            audit
                .scan_page(&head, PrivateScope::Chapter)
                .await
                .unwrap_err(),
            InventoryError::Unavailable
        );
    }
    let stopped = audit
        .progress(REVERSE_BUCKET, PrivateScope::Chapter)
        .await
        .unwrap()
        .unwrap();
    assert!(stopped.exhausted);
    assert_eq!(stopped.scanned, 0);
    assert_eq!(stopped.unknown_attempts, 8);
    assert!(audit
        .scan_page(&head, PrivateScope::Chapter)
        .await
        .unwrap()
        .is_none());
    assert!(
        audit
            .resume_exhausted(REVERSE_BUCKET, PrivateScope::Chapter)
            .await
            .unwrap(),
        "explicit operator retry releases the exhausted checkpoint"
    );
    let resumed = audit
        .scan_page(&head, PrivateScope::Chapter)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resumed.scanned, 128);
    assert!(!resumed.exhausted);
    assert_eq!(resumed.retries, 8);
    let still_registered: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM storage_objects WHERE bucket=$1")
            .bind(REVERSE_BUCKET)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(still_registered, 2501);
    pool.close().await;
}
