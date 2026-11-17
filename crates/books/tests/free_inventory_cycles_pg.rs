//! DROP-07c real disposable PG18: durable, fenced, read-only audit cursor.
//! No remote provider, S3 DELETE or production schema upgrade.
use sqlx::PgPool;
use tor_books::repositories::free_inventory_cycles::{
    InventoryCycleLease, InventoryDirection, InventoryNamespace, InventoryPageCounts,
    PgFreeInventoryCheckpoints,
};

#[tokio::test]
#[ignore = "requires isolated disposable PostgreSQL18 tor_ci"]
async fn drop_07c_reclaimable_checkpoint_is_exact_once_and_stale_lease_safe() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.contains("@127.0.0.1:") && url.ends_with("/tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let bucket = format!("tor-private-audit-{unique}");
    let repo = PgFreeInventoryCheckpoints::new(pool.clone());

    let mut lease: InventoryCycleLease = repo
        .claim(
            &bucket,
            InventoryDirection::ExpectedToS3,
            InventoryNamespace::Single,
            120,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.page_index, 0);
    assert_eq!(lease.epoch, 1);
    assert!(lease.continuation.is_none());
    // The immutable scope cannot be swapped onto another bucket or
    // namespace by a concurrent operator. Audit history is never deleted.
    assert!(
        sqlx::query("UPDATE free_storage_inventory_cycles SET namespace='pack' WHERE id=$1",)
            .bind(lease.id)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("DELETE FROM free_storage_inventory_cycles WHERE id=$1")
            .bind(lease.id)
            .execute(&pool)
            .await
            .is_err()
    );
    assert!(repo
        .claim(
            &bucket,
            InventoryDirection::ExpectedToS3,
            InventoryNamespace::Single,
            120,
        )
        .await
        .unwrap()
        .is_none());

    let initial = lease.clone();
    assert!(repo
        .acknowledge_page(
            &mut lease,
            Some("private/free-chapters/1/1.md"),
            InventoryPageCounts {
                inspected: 2,
                matched: 1,
                mismatch: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap());
    assert_eq!(lease.page_index, 1);
    assert_eq!(
        lease.continuation.as_deref(),
        Some("private/free-chapters/1/1.md")
    );

    let mut stale_cursor = initial;
    assert!(!repo
        .acknowledge_page(
            &mut stale_cursor,
            Some("private/free-chapters/1/1.md"),
            InventoryPageCounts {
                inspected: 2,
                matched: 2,
                ..Default::default()
            },
        )
        .await
        .unwrap());
    assert_eq!(
        stale_cursor.page_index, 0,
        "failed CAS never advances local state"
    );
    let total: (i64, i64, i64) = sqlx::query_as(
        "SELECT page_index,inspected_count,matched_count
         FROM free_storage_inventory_cycles WHERE id=$1",
    )
    .bind(lease.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(total, (1, 2, 1), "lost ACK replay cannot double count");

    let previous = lease;
    let mut lease = repo
        .claim(
            &bucket,
            InventoryDirection::ExpectedToS3,
            InventoryNamespace::Single,
            120,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(lease.id, previous.id);
    assert_eq!(lease.epoch, previous.epoch + 1);
    assert!(!repo.renew(&previous, 60).await.unwrap());

    assert!(repo
        .acknowledge_page(
            &mut lease,
            None,
            InventoryPageCounts {
                inspected: 1,
                unverifiable: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap());
    let status: (String, i64, i64) = sqlx::query_as(
        "SELECT status,page_index,inspected_count
         FROM free_storage_inventory_cycles WHERE id=$1",
    )
    .bind(lease.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, ("completed".to_owned(), 2, 3));
    assert!(!repo.renew(&lease, 60).await.unwrap());

    assert!(sqlx::query(
        "UPDATE free_storage_inventory_cycles SET completed_at=clock_timestamp() WHERE id=$1"
    )
    .bind(lease.id)
    .execute(&pool)
    .await
    .is_err());

    let mut second = repo
        .claim(
            &bucket,
            InventoryDirection::ExpectedToS3,
            InventoryNamespace::Single,
            120,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(repo
        .acknowledge_page(&mut second, None, InventoryPageCounts::default())
        .await
        .unwrap());
    let reports = repo
        .completed_reports(
            &bucket,
            InventoryDirection::ExpectedToS3,
            InventoryNamespace::Single,
        )
        .await
        .unwrap();
    assert_eq!(reports.len(), 2);
    assert_eq!((reports[0].id, reports[1].id), (second.id, lease.id));
    assert_eq!((reports[1].page_index, reports[1].inspected_count), (2, 3));
    assert_eq!(reports[1].unverifiable_count, 1);

    let mut retry = repo
        .claim(
            &bucket,
            InventoryDirection::ObservedToPg,
            InventoryNamespace::Pack,
            120,
        )
        .await
        .unwrap()
        .unwrap();
    sqlx::query(
        "UPDATE free_storage_inventory_cycles
         SET locked_until=clock_timestamp()-interval '1 second' WHERE id=$1",
    )
    .bind(retry.id)
    .execute(&pool)
    .await
    .unwrap();
    let old = retry.clone();
    retry = repo
        .claim(
            &bucket,
            InventoryDirection::ObservedToPg,
            InventoryNamespace::Pack,
            120,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(old.id, retry.id);
    assert_eq!(retry.epoch, old.epoch + 1);
    assert!(!repo.renew(&old, 60).await.unwrap());
    let mut old = old;
    assert!(!repo
        .acknowledge_page(
            &mut old,
            Some("opaque-token"),
            InventoryPageCounts {
                inspected: 1,
                unknown: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap());
    assert!(repo
        .acknowledge_page(
            &mut retry,
            Some("opaque-token"),
            InventoryPageCounts {
                inspected: 1,
                unknown: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap());
    assert_eq!(retry.page_index, 1);
    assert_eq!(retry.continuation.as_deref(), Some("opaque-token"));
    let mut retry = repo
        .claim(
            &bucket,
            InventoryDirection::ObservedToPg,
            InventoryNamespace::Pack,
            120,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(repo
        .acknowledge_page(&mut retry, None, InventoryPageCounts::default())
        .await
        .unwrap());
    assert_eq!(retry.page_index, 2);
    let no_delete: i64 =
        sqlx::query_scalar("SELECT count(*) FROM free_storage_inventory_cycles WHERE bucket=$1")
            .bind(&bucket)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(no_delete, 3);
    println!("DROP07C_PG_CHECKPOINT_PASS atomic cursor, one active, lost ACK, stale epoch, resume, complete, no delete");
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires isolated disposable PostgreSQL18 tor_ci"]
async fn drop_07c_checkpoints_allow_more_than_eight_pages_without_waiting() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.contains("@127.0.0.1:") && url.ends_with("/tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let bucket = format!("tor-private-audit-{unique}");
    let repo = PgFreeInventoryCheckpoints::new(pool.clone());
    let claim = || {
        repo.claim(
            &bucket,
            InventoryDirection::ObservedToPg,
            InventoryNamespace::Single,
            120,
        )
    };
    let mut lease = claim().await.unwrap().unwrap();
    let id = lease.id;
    for page in 1_i64..=10 {
        let token = format!("opaque-token-{page}");
        assert!(repo
            .acknowledge_page(
                &mut lease,
                Some(&token),
                InventoryPageCounts {
                    inspected: 1,
                    matched: 1,
                    ..Default::default()
                },
            )
            .await
            .unwrap());
        let next = claim().await.unwrap().unwrap();
        assert_eq!((next.id, next.page_index, next.epoch), (id, page, page + 1));
        assert_eq!(next.continuation.as_deref(), Some(token.as_str()));
        assert!(!repo.fail(&lease).await.unwrap());
        lease = next;
    }

    // An interrupted caller without progress can still exhaust its budget.
    for _ in 0..8 {
        sqlx::query(
            "UPDATE free_storage_inventory_cycles
             SET locked_until=clock_timestamp()-interval '1 second' WHERE id=$1",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
        match claim().await.unwrap() {
            Some(next) => assert_eq!((next.id, next.page_index), (id, 10)),
            None => break,
        }
    }
    let persisted: (String, i32, i64, i64) = sqlx::query_as(
        "SELECT status,attempts_since_progress,page_index,inspected_count
         FROM free_storage_inventory_cycles WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(persisted, ("failed".to_owned(), 8, 10, 10));
}
