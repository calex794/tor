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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable PostgreSQL18 tor_ci"]
async fn managed_queue_eof_waits_for_http_drained_witness() {
    let f = Fixture::new().await;
    let accounts = accounts(&f).await;
    let (runtime, mut worker) = Runtime::new(accounts, ORIGIN.into(), false, None);
    worker.prepare().await.unwrap();
    let epoch = latest(&f);
    let issuer = runtime.issuer();
    let app = runtime.clone().attach(Router::new());
    let collector = worker.spawn_managed();
    let probe = collector.probe();
    assert!(issuer.issue(f.card()).is_some());
    // The actual server drops its Router before sending the collector witness.
    // Drop every sender for this worker, including the retained Runtime clone.
    drop(issuer);
    drop(runtime);
    drop(app);
    tokio::time::timeout(Duration::from_secs(3), async {
        while probe.snapshot().ready {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("worker did not observe queue EOF");
    assert_eq!(probe.snapshot().queued, 1);
    assert_eq!(probe.snapshot().persisted, 1);
    assert_eq!(
        sql(
            &f.url,
            &format!("SELECT retired FROM flow_observation_epochs WHERE epoch={epoch}")
        ),
        "f",
        "queue EOF is not proof that the HTTP owner supplied its stop witness"
    );
    assert!(
        collector.shutdown_after_http_drained().await,
        "managed worker must remain available to consume the later witness"
    );
    assert!(!probe.snapshot().ready);
    assert_eq!(
        sql(
            &f.url,
            &format!("SELECT retired FROM flow_observation_epochs WHERE epoch={epoch}")
        ),
        "t"
    );
    f.cleanup().await;
}

// #1134: reproduce the real PostgreSQL hourly aggregate lock that previously
// disabled observation callbacks in the browser chain. No restart, fake ACK,
// or retry-until-green is allowed: one healthy callback must succeed after the
// collector's next durable checkpoint, and failed requests must not count.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable PostgreSQL18 tor_ci"]
async fn hourly_aggregate_lock_fails_closed_then_same_collector_recovers_exactly_once() {
    let f = Fixture::new().await;
    let proof = f.proof().await;
    let epoch = latest(&f);
    let rendered = payload(&proof, "rendered").to_string();
    let viewed = payload(&proof, "view").to_string();

    // A separate real PG connection holds ACCESS EXCLUSIVE while the web
    // instance continues running with its original three-connection pool.
    let lock_pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&f.url)
        .await
        .unwrap();
    let mut blocker = lock_pool.begin().await.unwrap();
    sqlx::query("LOCK TABLE flow_observation_hourly IN ACCESS EXCLUSIVE MODE")
        .execute(&mut *blocker)
        .await
        .unwrap();

    let rejected = tokio::time::timeout(
        Duration::from_secs(2),
        f.send("POST", API, "", ORIGIN, &rendered),
    )
    .await
    .expect("a blocked hourly aggregate must not hang the HTTP reader pool");
    private(&rejected, StatusCode::SERVICE_UNAVAILABLE);
    assert!(
        !f.runtime.queue_snapshot().ready,
        "503 must fail closed instead of leaving issuance enabled"
    );
    private(
        &f.send("POST", API, "", ORIGIN, &viewed).await,
        StatusCode::SERVICE_UNAVAILABLE,
    );

    blocker.rollback().await.unwrap();
    // SQL writes in the aborted HTTP transaction must all roll back, including
    // proof event flags. The original persisted opportunity remains untouched.
    assert_eq!(
        sql(
            &f.url,
            &format!(
                "SELECT opportunities||','||rendered||','||views||','||clicks||','||replayed FROM flow_observation_hourly WHERE book_id={}",
                f.book
            )
        ),
        "1,0,0,0,0",
        "failed callbacks must not invent a rendered card or view"
    );
    tokio::time::timeout(Duration::from_secs(1), f.store.describe(&proof))
        .await
        .expect("the reader pool must remain usable after the lock")
        .expect("the original proof must remain valid");

    // Wait for a fresh *real* collector health acknowledgement, not repeated
    // observation POSTs. A dead collector fails this strict deadline.
    tokio::time::timeout(Duration::from_secs(12), async {
        while !f.runtime.queue_snapshot().ready {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the original collector never recovered its database lease");
    assert_eq!(
        latest(&f),
        epoch,
        "the short SQL lock must recover within the existing collector epoch"
    );

    private(
        &f.send("POST", API, "", ORIGIN, &rendered).await,
        StatusCode::NO_CONTENT,
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    private(
        &f.send("POST", API, "", ORIGIN, &viewed).await,
        StatusCode::NO_CONTENT,
    );
    private(
        &f.send("POST", API, "", ORIGIN, &viewed).await,
        StatusCode::NO_CONTENT,
    );
    private(
        &f.send("POST", API, "", "https://evil.invalid", &viewed)
            .await,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        sql(
            &f.url,
            &format!(
                "SELECT opportunities||','||rendered||','||views||','||clicks||','||replayed FROM flow_observation_hourly WHERE book_id={}",
                f.book
            )
        ),
        "1,1,1,0,1",
        "a recovered callback and replay must count only one durable view"
    );
    assert_eq!(
        sql(
            &f.url,
            &format!(
                "SELECT count(*) FROM rec_subjects WHERE user_id={}",
                f.actor
            )
        ),
        "0",
        "anonymous observation must not create a reader identity"
    );
    lock_pool.close().await;
    f.cleanup().await;
}
