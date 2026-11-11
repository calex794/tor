//! Actual managed worker, HTTP fences and disabled historical reads.
use super::*;

async fn accounts(f: &Fixture) -> PgAccounts {
    let catalog = PgCatalog::connect(&f.url).await.unwrap();
    PgAccounts::from_pool(catalog.pool().clone())
}
fn latest(f: &Fixture) -> i64 {
    sql(&f.url, "SELECT max(epoch) FROM flow_observation_epochs")
        .parse()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable PostgreSQL18 tor_ci"]
async fn managed_stop_drains_queued_cards_before_retiring_the_epoch() {
    let f = Fixture::new().await;
    let accounts = accounts(&f).await;
    let (runtime, mut worker) = Runtime::new(accounts.clone(), ORIGIN.into(), false, None);
    worker.prepare().await.unwrap();
    let epoch = latest(&f);
    let issuer = runtime.issuer();
    let collector = worker.spawn_managed();
    let probe = collector.probe();
    for _ in 0..64 {
        assert!(issuer.issue(f.card()).is_some());
    }
    // These producers have returned; this fixture has no in-flight HTTP request.
    assert!(collector.shutdown_after_http_drained().await);
    assert!(!probe.snapshot().ready);
    assert_eq!(probe.snapshot().queued, 64);
    assert_eq!(probe.snapshot().persisted, 64);
    assert_eq!(
        sql(
            &f.url,
            &format!("SELECT retired FROM flow_observation_epochs WHERE epoch={epoch}")
        ),
        "t"
    );
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable PostgreSQL18 tor_ci"]
async fn forced_abort_disables_issuance_and_keeps_the_epoch_unresolved() {
    let f = Fixture::new().await;
    let accounts = accounts(&f).await;
    let (runtime, mut worker) = Runtime::new(accounts.clone(), ORIGIN.into(), false, None);
    worker.prepare().await.unwrap();
    let epoch = latest(&f);
    let collector = worker.spawn_managed();
    let probe = collector.probe();
    collector.abort();
    assert!(!probe.snapshot().ready);
    assert!(runtime.issuer().issue(f.card()).is_none());
    assert_eq!(
        sql(
            &f.url,
            &format!("SELECT retired FROM flow_observation_epochs WHERE epoch={epoch}")
        ),
        "f",
        "abort is not evidence of drained HTTP ingress"
    );
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable PostgreSQL18 tor_ci"]
async fn expired_database_epoch_returns_503_and_persists_the_known_failure_on_handoff() {
    let f = Fixture::new().await;
    let proof = f.proof().await;
    let accounts = accounts(&f).await;
    let (runtime, mut parked) = Runtime::new(accounts.clone(), ORIGIN.into(), false, None);
    parked.prepare().await.unwrap();
    let epoch = latest(&f);
    let app = runtime.clone().attach(Router::new());
    sql(
        &f.url,
        &format!(
            "UPDATE flow_observation_epochs SET lease_until_ms=confirmed_ms WHERE epoch={epoch}"
        ),
    );
    private(
        &send(
            &app,
            "POST",
            API,
            "",
            ORIGIN,
            &payload(&proof, "rendered").to_string(),
        )
        .await,
        StatusCode::SERVICE_UNAVAILABLE,
    );
    assert!(!runtime.queue_snapshot().ready);
    assert_eq!(runtime.queue_snapshot().failed, 1);
    assert!(runtime.issuer().issue(f.card()).is_none());
    assert_eq!(
        parked.prepare().await,
        Err(tor_recommendations::domain::exposure::ExposureError::Expired)
    );
    parked.prepare().await.unwrap();
    let next = latest(&f);
    assert_ne!(next, epoch);
    assert_eq!(sql(&f.url, &format!("SELECT coalesce(bool_or(failed),false) FROM flow_observation_health_hourly WHERE epoch={next}")), "t");
    // A dropped worker is deliberately unresolved; no false graceful witness.
    drop(parked);
    assert!(!runtime.queue_snapshot().ready);
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable PostgreSQL18 tor_ci"]
async fn unavailable_collector_keeps_admin_history_and_rejects_callbacks() {
    let f = Fixture::new().await;
    let proof = f.proof().await;
    let accounts = accounts(&f).await;
    sql(
        &f.url,
        &format!(
            "INSERT INTO account_roles(user_id,role) VALUES({},'administrator')",
            f.actor
        ),
    );
    let (runtime, parked) = Runtime::new(accounts, ORIGIN.into(), false, None);
    let app = runtime.attach_routes(Router::new());
    private(
        &send(
            &app,
            "POST",
            API,
            "",
            ORIGIN,
            &payload(&proof, "rendered").to_string(),
        )
        .await,
        StatusCode::SERVICE_UNAVAILABLE,
    );
    private(
        &send(&app, "GET", REPORT, &f.cookie, "", "").await,
        StatusCode::OK,
    );
    drop(parked);
    f.cleanup().await;
}
