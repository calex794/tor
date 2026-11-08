//! G-06/G-07 shared outbox: leases, independent consumers and stale-worker fencing.
use sqlx::PgPool;
use tor_ingest::repositories::outbox::{FailureDisposition, OutboxError, PgOutbox};

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
    sqlx::query(
        "UPDATE outbox_events
         SET next_attempt_at=now()-interval '10 days'
         WHERE id=$1::uuid",
    )
    .bind(&event_id)
    .execute(&pool)
    .await
    .unwrap();
    let outbox = PgOutbox::new(pool.clone());
    let claimed = outbox
        .claim_next_for_consumer("G06Test", "g06-one")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(claimed.id, event_id);
    assert_eq!(claimed.required_consumers, vec!["g06-one", "g06-two"]);
    assert_eq!(
        outbox
            .acknowledge(&claimed.id, claimed.lease_epoch, "unknown")
            .await,
        Err(OutboxError::Invalid)
    );
    assert!(!outbox
        .acknowledge(&claimed.id, claimed.lease_epoch, "g06-one")
        .await
        .unwrap());
    let delivered: Option<String> =
        sqlx::query_scalar("SELECT delivered_at::text FROM outbox_events WHERE id=$1::uuid")
            .bind(&event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        delivered.is_none(),
        "first consumer cannot finish all consumers"
    );
    assert!(!outbox
        .acknowledge(&claimed.id, claimed.lease_epoch, "g06-one")
        .await
        .unwrap());
    assert!(outbox
        .acknowledge(&claimed.id, claimed.lease_epoch, "g06-two")
        .await
        .unwrap());
    let receipts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM event_deliveries WHERE event_id=$1::uuid")
            .bind(&event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(receipts, 2, "replay did not duplicate delivery");
    assert_eq!(
        outbox
            .acknowledge(&claimed.id, claimed.lease_epoch, "g06-two")
            .await,
        Err(OutboxError::LostLease)
    );
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
    sqlx::query(
        "UPDATE outbox_events
         SET next_attempt_at=now()-interval '10 days'
         WHERE id=$1::uuid",
    )
    .bind(&event_id)
    .execute(&pool)
    .await
    .unwrap();
    let outbox = PgOutbox::new(pool.clone());
    let first = outbox.claim_next_for_consumer("G07Test", "g07-target").await.unwrap().unwrap();
    assert_eq!(first.id, event_id);
    sqlx::query(
        "UPDATE outbox_events SET lease_until=now()-interval '1 second'
         WHERE id=$1::uuid",
    )
    .bind(&event_id)
    .execute(&pool)
    .await
    .unwrap();
    let second = outbox.claim_next_for_consumer("G07Test", "g07-target").await.unwrap().unwrap();
    assert_eq!(second.id, event_id);
    assert_eq!(second.lease_epoch, first.lease_epoch + 1);
    assert_eq!(
        outbox
            .acknowledge(&event_id, first.lease_epoch, "g07-target")
            .await,
        Err(OutboxError::LostLease)
    );
    assert!(outbox
        .acknowledge(&event_id, second.lease_epoch, "g07-target")
        .await
        .unwrap());
    let receipt_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM event_deliveries WHERE event_id=$1::uuid")
            .bind(&event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(receipt_count, 1);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL /tor_ci"]
async fn g_07_retry_preserves_successful_consumer_receipt() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "never touch a durable database");
    let pool = PgPool::connect(&url).await.unwrap();
    let event_id: String = sqlx::query_scalar(
        "INSERT INTO outbox_events
         (event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,
          payload,required_consumers,next_attempt_at)
         VALUES('G07RetryTest',1,'book',0,$1,'{}'::jsonb,
                ARRAY['g07-one','g07-two'],now()-interval '10 days')
         RETURNING id::text",
    )
    .bind(format!("g07-retry-test-{}", std::process::id()))
    .fetch_one(&pool)
    .await
    .unwrap();

    let outbox = PgOutbox::new(pool.clone());
    let first = outbox.claim_next_for_consumer("G07RetryTest", "g07-one").await.unwrap().unwrap();
    assert_eq!(first.id, event_id);
    assert!(!outbox
        .acknowledge(&event_id, first.lease_epoch, "g07-one")
        .await
        .unwrap());

    assert_eq!(
        outbox
            .record_failure(&event_id, first.lease_epoch, true)
            .await,
        Ok(FailureDisposition::RetryScheduled)
    );
    let state: (i32, f64, bool, bool) = sqlx::query_as(
        "SELECT attempts,EXTRACT(EPOCH FROM (next_attempt_at-now()))::double precision,
                lease_until IS NULL,dead_at IS NULL
         FROM outbox_events WHERE id=$1::uuid",
    )
    .bind(&event_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(state.0, 1);
    assert!(
        state.1 > 3.0 && state.1 <= 10.0,
        "retry uses 5..9s plus elapsed time"
    );
    assert!(
        state.2 && state.3,
        "retry releases lease without dead-lettering"
    );
    assert_eq!(
        outbox
            .acknowledge(&event_id, first.lease_epoch, "g07-two")
            .await,
        Err(OutboxError::LostLease)
    );

    // Advance only the isolated test event; no wall-clock sleep.
    sqlx::query(
        "UPDATE outbox_events SET next_attempt_at=now()-interval '10 days'
         WHERE id=$1::uuid",
    )
    .bind(&event_id)
    .execute(&pool)
    .await
    .unwrap();
    let second = outbox.claim_next_for_consumer("G07RetryTest", "g07-two").await.unwrap().unwrap();
    assert_eq!(second.id, event_id);
    assert_eq!(second.lease_epoch, first.lease_epoch + 1);
    assert!(outbox
        .acknowledge(&event_id, second.lease_epoch, "g07-two")
        .await
        .unwrap());
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM event_deliveries WHERE event_id=$1::uuid")
            .bind(&event_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 2, "completed consumer is not delivered again");
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL /tor_ci"]
async fn g_07_dead_letters_irrecoverable_and_exhausted_claims() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "never touch a durable database");
    let pool = PgPool::connect(&url).await.unwrap();
    let outbox = PgOutbox::new(pool.clone());

    let first_id: String = sqlx::query_scalar(
        "INSERT INTO outbox_events
         (event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,
          payload,required_consumers,next_attempt_at)
         VALUES('G07DeadTest',1,'book',0,$1,'{}'::jsonb,
                ARRAY['g07-failed'],now()-interval '10 days')
         RETURNING id::text",
    )
    .bind(format!("g07-dead-test-{}", std::process::id()))
    .fetch_one(&pool)
    .await
    .unwrap();
    let first = outbox.claim_next_for_consumer("G07DeadTest", "g07-failed").await.unwrap().unwrap();
    assert_eq!(first.id, first_id);
    assert_eq!(
        outbox
            .record_failure(&first_id, first.lease_epoch, false)
            .await,
        Ok(FailureDisposition::DeadLetter)
    );
    assert_eq!(
        outbox
            .acknowledge(&first_id, first.lease_epoch, "g07-failed")
            .await,
        Err(OutboxError::LostLease)
    );

    let last_id: String = sqlx::query_scalar(
        "INSERT INTO outbox_events
         (event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,
          payload,required_consumers,next_attempt_at,attempts)
         VALUES('G07LastTest',1,'book',0,$1,'{}'::jsonb,
                ARRAY['g07-last'],now()-interval '10 days',11)
         RETURNING id::text",
    )
    .bind(format!("g07-last-test-{}", std::process::id()))
    .fetch_one(&pool)
    .await
    .unwrap();
    let last = outbox.claim_next_for_consumer("G07LastTest", "g07-last").await.unwrap().unwrap();
    assert_eq!(last.id, last_id);
    assert_eq!(
        outbox
            .record_failure(&last_id, last.lease_epoch, true)
            .await,
        Ok(FailureDisposition::DeadLetter),
        "the twelfth failed claim cannot be scheduled forever"
    );

    let crashed_id: String = sqlx::query_scalar(
        "INSERT INTO outbox_events
         (event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,
          payload,required_consumers,next_attempt_at,attempts,lease_until)
         VALUES('G07CrashTest',1,'book',0,$1,'{}'::jsonb,
                ARRAY['g07-crashed'],now()-interval '10 days',12,
                now()-interval '1 second')
         RETURNING id::text",
    )
    .bind(format!("g07-crashed-test-{}", std::process::id()))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(outbox.reap_exhausted().await.unwrap() >= 1);
    for event_id in [&first_id, &last_id, &crashed_id] {
        let dead: bool =
            sqlx::query_scalar("SELECT dead_at IS NOT NULL FROM outbox_events WHERE id=$1::uuid")
                .bind(event_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(dead, "failed and crashed events must become dead letters");
    }
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL /tor_ci"]
async fn g_07_claim_is_consumer_scoped_and_handoff_fences_old_worker() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "never touch a durable database");
    let pool = PgPool::connect(&url).await.unwrap();
    let id: String = sqlx::query_scalar(
        "INSERT INTO outbox_events
         (event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,
          payload,required_consumers,next_attempt_at)
         VALUES('G07ScopedTest',1,'book',0,$1,'{}'::jsonb,
                ARRAY['g07-scope-a','g07-scope-b'],now()-interval '10 days')
         RETURNING id::text",
    )
    .bind(format!("g07-scope-{}", std::process::id()))
    .fetch_one(&pool)
    .await
    .unwrap();
    let unrelated: String = sqlx::query_scalar(
        "INSERT INTO outbox_events
         (event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,
          payload,required_consumers,next_attempt_at,attempts)
         VALUES('G07UnrelatedTest',1,'book',0,$1,'{}'::jsonb,
                ARRAY['different-consumer'],now()-interval '10 days',12)
         RETURNING id::text",
    )
    .bind(format!("g07-unrelated-{}", std::process::id()))
    .fetch_one(&pool)
    .await
    .unwrap();
    let repo = PgOutbox::new(pool.clone());
    assert!(repo
        .claim_next_for_consumer("G07ScopedTest", "other")
        .await
        .unwrap()
        .is_none());
    assert!(repo
        .claim_next_for_consumer("G07UnrelatedTest", "g07-scope-a")
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        repo.claim_next_for_consumer("", "g07-scope-a").await,
        Err(OutboxError::Invalid)
    );
    let a = repo
        .claim_next_for_consumer("G07ScopedTest", "g07-scope-a")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(a.id, id);
    assert!(!repo
        .acknowledge(&id, a.lease_epoch, "g07-scope-a")
        .await
        .unwrap());
    repo.release_unfinished(&id, a.lease_epoch).await.unwrap();
    assert_eq!(
        repo.renew_lease(&id, a.lease_epoch).await,
        Err(OutboxError::LostLease)
    );
    assert_eq!(
        repo.acknowledge(&id, a.lease_epoch, "g07-scope-b").await,
        Err(OutboxError::LostLease)
    );
    assert!(repo
        .claim_next_for_consumer("G07ScopedTest", "g07-scope-a")
        .await
        .unwrap()
        .is_none());
    let b = repo
        .claim_next_for_consumer("G07ScopedTest", "g07-scope-b")
        .await
        .unwrap()
        .unwrap();
    assert!(b.lease_epoch > a.lease_epoch);
    assert!(repo
        .acknowledge(&id, b.lease_epoch, "g07-scope-b")
        .await
        .unwrap());
    let dead: bool =
        sqlx::query_scalar("SELECT dead_at IS NOT NULL FROM outbox_events WHERE id=$1::uuid")
            .bind(&unrelated)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!dead, "scoped claim must not reap unrelated events");
    sqlx::query("DELETE FROM outbox_events WHERE id=$1::uuid")
        .bind(&unrelated)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL /tor_ci"]
async fn g_07_renewal_rejects_expired_and_replaced_generations() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "never touch a durable database");
    let pool = PgPool::connect(&url).await.unwrap();
    let id: String = sqlx::query_scalar(
        "INSERT INTO outbox_events
         (event_type,schema_version,aggregate_kind,aggregate_id,dedupe_key,
          payload,required_consumers,next_attempt_at)
         VALUES('G07HeartbeatTest',1,'book',0,$1,'{}'::jsonb,
                ARRAY['g07-heartbeat'],now()-interval '10 days')
         RETURNING id::text",
    )
    .bind(format!("g07-heartbeat-{}", std::process::id()))
    .fetch_one(&pool)
    .await
    .unwrap();
    let repo = PgOutbox::new(pool.clone());
    let a = repo
        .claim_next_for_consumer("G07HeartbeatTest", "g07-heartbeat")
        .await
        .unwrap()
        .unwrap();
    repo.renew_lease(&id, a.lease_epoch).await.unwrap();
    sqlx::query(
        "UPDATE outbox_events SET lease_until=now()-interval '1 second'
         WHERE id=$1::uuid",
    )
    .bind(&id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        repo.renew_lease(&id, a.lease_epoch).await,
        Err(OutboxError::LostLease)
    );
    let b = repo
        .claim_next_for_consumer("G07HeartbeatTest", "g07-heartbeat")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(b.lease_epoch, a.lease_epoch + 1);
    assert_eq!(
        repo.release_unfinished(&id, a.lease_epoch).await,
        Err(OutboxError::LostLease)
    );
    assert_eq!(
        repo.record_failure(&id, a.lease_epoch, true).await,
        Err(OutboxError::LostLease)
    );
    assert_eq!(
        repo.record_failure(&id, b.lease_epoch, false).await,
        Ok(FailureDisposition::DeadLetter)
    );
    assert!(repo
        .claim_next_for_consumer("G07HeartbeatTest", "g07-heartbeat")
        .await
        .unwrap()
        .is_none());
    pool.close().await;
}
