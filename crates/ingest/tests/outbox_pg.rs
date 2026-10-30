//! G-06/G-07 shared outbox: leases, independent consumers and stale-worker fencing.
use sqlx::PgPool;
use tor_ingest::repositories::outbox::{OutboxError, PgOutbox};

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL /tor_ci"]
async fn g_06_two_consumers_finish_only_after_both_receipts() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "never touch a durable database");
    let pool = PgPool::connect(&url).await.unwrap();
    let event_id: String = sqlx::query_scalar(
        "INSERT INTO outbox_events
         (event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers)
         VALUES('G06Test',1,'book',0,$1,'{}'::jsonb,ARRAY['g06-one','g06-two'])
         RETURNING id::text",
    )
    .bind(format!("g06-test-{}", std::process::id()))
    .fetch_one(&pool)
    .await
    .unwrap();
    let outbox = PgOutbox::new(pool.clone());
    let claimed = outbox.claim_next().await.unwrap().unwrap();
    assert_eq!(claimed.id, event_id);
    assert_eq!(claimed.required_consumers, vec!["g06-one", "g06-two"]);
    assert_eq!(outbox.acknowledge(&claimed.id, claimed.lease_epoch, "unknown").await, Err(OutboxError::Invalid));
    assert!(!outbox.acknowledge(&claimed.id, claimed.lease_epoch, "g06-one").await.unwrap());
    let delivered: Option<String> = sqlx::query_scalar(
        "SELECT delivered_at::text FROM outbox_events WHERE id=$1::uuid",
    )
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(delivered.is_none(), "first consumer cannot finish all consumers");
    assert!(!outbox.acknowledge(&claimed.id, claimed.lease_epoch, "g06-one").await.unwrap());
    assert!(outbox.acknowledge(&claimed.id, claimed.lease_epoch, "g06-two").await.unwrap());
    let receipts: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM event_deliveries WHERE event_id=$1::uuid",
    )
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipts, 2, "replay did not duplicate delivery");
    assert_eq!(outbox.acknowledge(&claimed.id, claimed.lease_epoch, "g06-two").await, Err(OutboxError::LostLease));
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL /tor_ci"]
async fn g_07_expired_worker_cannot_ack_after_reclaim() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "never touch a durable database");
    let pool = PgPool::connect(&url).await.unwrap();
    let event_id: String = sqlx::query_scalar(
        "INSERT INTO outbox_events
         (event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,payload,required_consumers)
         VALUES('G07Test',1,'book',0,$1,'{}'::jsonb,ARRAY['g07-target'])
         RETURNING id::text",
    )
    .bind(format!("g07-test-{}", std::process::id()))
    .fetch_one(&pool)
    .await
    .unwrap();
    let outbox = PgOutbox::new(pool.clone());
    let first = outbox.claim_next().await.unwrap().unwrap();
    assert_eq!(first.id, event_id);
    sqlx::query(
        "UPDATE outbox_events SET lease_until=now()-interval '1 second'
         WHERE id=$1::uuid",
    )
    .bind(&event_id)
    .execute(&pool)
    .await
    .unwrap();
    let second = outbox.claim_next().await.unwrap().unwrap();
    assert_eq!(second.id, event_id);
    assert_eq!(second.lease_epoch, first.lease_epoch + 1);
    assert_eq!(outbox.acknowledge(&event_id, first.lease_epoch, "g07-target").await, Err(OutboxError::LostLease));
    assert!(outbox.acknowledge(&event_id, second.lease_epoch, "g07-target").await.unwrap());
    let receipt_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM event_deliveries WHERE event_id=$1::uuid",
    )
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipt_count, 1);
    pool.close().await;
}
