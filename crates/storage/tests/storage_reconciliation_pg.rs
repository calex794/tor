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

    let registry_objects: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM storage_objects WHERE bucket=$1",
    )
    .bind(BUCKET)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(registry_objects, 0, "inventory never writes the live registry");
    pool.close().await;
}
