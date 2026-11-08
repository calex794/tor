//! AB-02: real PostgreSQL locks and distinct-target accounting across pools.
//! Requires a freshly migrated disposable tor_ci, never production.
use std::time::{SystemTime, UNIX_EPOCH};
use tor_accounts::{repositories::PgAccounts, services::auth_observation::AuthObservationSigner};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires migrated disposable PostgreSQL 18 tor_ci"]
async fn a09_spray_shared_atomic_locks_and_nat_safety() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test PostgreSQL URL");
    assert!(url.contains("127.0.0.1") && url.ends_with("/tor_ci"));
    let pool_a = sqlx::PgPool::connect(&url).await.unwrap();
    let pool_b = sqlx::PgPool::connect(&url).await.unwrap();
    let first = PgAccounts::from_pool(pool_a.clone());
    let second = PgAccounts::from_pool(pool_b.clone());
    let signer = AuthObservationSigner::from_hex(&"a7".repeat(32)).unwrap();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let (ip, subnet) = signer.source("198.51.100.27".parse().unwrap());

    // Repeated failures for one account are not spray; 19 failures against
    // 11 accounts must also stay below the minimum-20/minimum-12 gate.
    for i in 0..19 {
        let target = signer.target(&format!("spray-{stamp}-{}@example.invalid", i % 11));
        let adapter = if i % 2 == 0 { &first } else { &second };
        adapter
            .record_spray_failure(&ip, &subnet, &target, true)
            .await
            .unwrap();
    }
    assert!(second.spray_retry_after(&ip).await.unwrap().is_none());
    let last = signer.target(&format!("spray-{stamp}-11@example.invalid"));
    first
        .record_spray_failure(&ip, &subnet, &last, true)
        .await
        .unwrap();
    let remaining = second.spray_retry_after(&ip).await.unwrap().unwrap();
    assert!((1..=90).contains(&remaining));

    let counts: (i32, i32) = sqlx::query_as(
        "SELECT failed_attempts,distinct_targets FROM auth_spray_windows
         WHERE scope='ip' AND source_key=$1",
    )
    .bind(ip.as_slice())
    .fetch_one(&pool_a)
    .await
    .unwrap();
    assert_eq!(counts, (20, 12));

    // Another verified address on the same /24 has no lock, even though
    // subnet observations have exceeded the 12-distinct trigger.
    let (neighbour, shared_subnet) = signer.source("198.51.100.28".parse().unwrap());
    assert_eq!(shared_subnet, subnet);
    assert!(first.spray_retry_after(&neighbour).await.unwrap().is_none());
    let neighbour_target = signer.target(&format!("ordinary-{stamp}@example.invalid"));
    second
        .record_spray_failure(&neighbour, &shared_subnet, &neighbour_target, true)
        .await
        .unwrap();
    let subnet_locked: Option<String> = sqlx::query_scalar(
        "SELECT locked_until::text FROM auth_spray_windows
         WHERE scope='subnet' AND source_key=$1",
    )
    .bind(subnet.as_slice())
    .fetch_one(&pool_a)
    .await
    .unwrap();
    assert!(subnet_locked.is_none());

    // Observe mode records a warning but never enforces a lock.
    let (observe_ip, observe_net) = signer.source("198.51.101.27".parse().unwrap());
    for i in 0..25 {
        let target = signer.target(&format!("observe-{stamp}-{i}@example.invalid"));
        first
            .record_spray_failure(&observe_ip, &observe_net, &target, false)
            .await
            .unwrap();
    }
    assert!(second
        .spray_retry_after(&observe_ip)
        .await
        .unwrap()
        .is_none());

    // Two independent PgPools race on the same source; target increments
    // are not lost or double-counted (the source row is locked in PostgreSQL).
    let (race_ip, race_net) = signer.source("198.51.102.27".parse().unwrap());
    let mut jobs = tokio::task::JoinSet::new();
    for i in 0..24 {
        let adapter = if i % 2 == 0 {
            first.clone()
        } else {
            second.clone()
        };
        let target = signer.target(&format!("race-{stamp}-{i}@example.invalid"));
        jobs.spawn(async move {
            adapter
                .record_spray_failure(&race_ip, &race_net, &target, true)
                .await
        });
    }
    while let Some(result) = jobs.join_next().await {
        result.unwrap().unwrap();
    }
    let race_counts: (i32, i32) = sqlx::query_as(
        "SELECT failed_attempts,distinct_targets FROM auth_spray_windows
         WHERE scope='ip' AND source_key=$1",
    )
    .bind(race_ip.as_slice())
    .fetch_one(&pool_b)
    .await
    .unwrap();
    assert_eq!(race_counts, (24, 24));
    assert!(first.spray_retry_after(&race_ip).await.unwrap().is_some());

    // Window reset removes the old unique-target set and expires the lock.
    sqlx::query(
        "UPDATE auth_spray_windows
         SET period_started=clock_timestamp()-INTERVAL '11 minutes',
             locked_until=clock_timestamp()-INTERVAL '1 second'
         WHERE scope='ip' AND source_key=$1",
    )
    .bind(ip.as_slice())
    .execute(&pool_a)
    .await
    .unwrap();
    first
        .record_spray_failure(&ip, &subnet, &last, true)
        .await
        .unwrap();
    let reset_counts: (i32, i32) = sqlx::query_as(
        "SELECT failed_attempts,distinct_targets FROM auth_spray_windows
         WHERE scope='ip' AND source_key=$1",
    )
    .bind(ip.as_slice())
    .fetch_one(&pool_b)
    .await
    .unwrap();
    assert_eq!(reset_counts, (1, 1));
    assert!(second.spray_retry_after(&ip).await.unwrap().is_none());

    // Bounded maintenance deletes expired state and cascading target rows.
    sqlx::query(
        "UPDATE auth_spray_windows
         SET touched_at=clock_timestamp()-INTERVAL '2 days'
         WHERE scope='ip' AND source_key=$1",
    )
    .bind(ip.as_slice())
    .execute(&pool_a)
    .await
    .unwrap();
    first.prune_security_data().await.unwrap();
    let stale: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_spray_targets WHERE scope='ip' AND source_key=$1",
    )
    .bind(ip.as_slice())
    .fetch_one(&pool_b)
    .await
    .unwrap();
    assert_eq!(stale, 0);

    // Pseudonyms have no plaintext network or account data.
    let leaked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_spray_windows
         WHERE source_key::text ILIKE '%198.51.100%'
            OR source_key::text ILIKE '%example.invalid%'",
    )
    .fetch_one(&pool_a)
    .await
    .unwrap();
    assert_eq!(leaked, 0);
}
