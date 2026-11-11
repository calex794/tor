//! Real A/F HTTP and PostgreSQL; tokens come only from the trusted server issuer.
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use serde_json::{json, Value};
use std::{
    num::NonZeroU32,
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_contracts::exposure::*;
use tor_reading::repositories::PgReader;
use tor_recommendations::repositories::exposure::PgExposure;
use tor_web::flow_exposure::Runtime;
use tower::ServiceExt;
const ORIGIN: &str = "http://127.0.0.1:3109";
const API: &str = "/api/v1/flow/observations";
const REPORT: &str = "/admin/audience/exposure";

struct Guard {
    allowed: AtomicBool,
    expected: CardObservation,
}
#[async_trait::async_trait]
impl EditorialObservationGuard for Guard {
    async fn allowed(&self, card: &CardObservation) -> Result<bool, ObservationGuardError> {
        assert_eq!(*card, self.expected);
        Ok(self.allowed.load(Ordering::Relaxed))
    }
    async fn campaign_book(&self, campaign_id: i64) -> Result<Option<i64>, ObservationGuardError> {
        Ok(
            (self.expected.editorial.unwrap().campaign_id == campaign_id)
                .then_some(self.expected.book_id),
        )
    }
}
struct Fixture {
    url: String,
    app: Router,
    actor: i64,
    cookie: String,
    book: i64,
    issuer: Arc<dyn ObservationIssuer>,
    store: PgExposure,
    runtime: Runtime,
    worker: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL").expect("disposable tor_ci");
        assert!(
            url.starts_with("postgresql://tor_app:")
                && url.contains(&format!(
                    "@127.0.0.1:{}/",
                    std::env::var("FLOW_TEST_PG_PORT").unwrap_or_else(|_| "5432".into())
                ))
                && url.ends_with("/tor_ci")
        );
        let catalog = PgCatalog::connect(&url).await.unwrap();
        let accounts = PgAccounts::from_pool(catalog.pool().clone());
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let token = generate_session_token();
        let actor = accounts
            .register_with_session(
                &format!("exposure-http-{stamp}@tor-ci.example"),
                "Observation operator",
                "$argon2id$test",
                &session_token_digest(&token),
                NonZeroU32::new(3600).unwrap(),
            )
            .await
            .unwrap();
        let book=sql(&url,&format!("INSERT INTO books(source_identity,target_language,title,visibility,access_level) VALUES('ci:exposure-http:{stamp}','ru','Observation <script>safe</script>','public','basic') RETURNING id")).parse().unwrap();
        let store = PgExposure::new(catalog.pool().clone());
        let app = tor_web::router_with_postgres_reading_auth(
            catalog.clone(),
            PgReader::new(catalog.pool().clone()),
            accounts.clone(),
            false,
            ORIGIN.into(),
        );
        let (runtime, mut worker) = Runtime::new(accounts, ORIGIN.into(), false, None);
        worker.prepare().await.unwrap();
        let issuer = runtime.issuer();
        let app = runtime.clone().attach(app);
        let worker = tokio::spawn(worker.run());
        Self {
            url,
            app,
            actor,
            cookie: format!("tor-dev-session={token}"),
            book,
            issuer,
            store,
            runtime,
            worker,
        }
    }
    fn card(&self) -> CardObservation {
        CardObservation {
            book_id: self.book,
            surface: ExposureSurface::Catalog,
            editorial: None,
        }
    }
    async fn proof(&self) -> String {
        let proof = self.issuer.issue(self.card()).unwrap();
        for _ in 0..100 {
            if self.store.describe(&proof).await.is_ok() {
                return proof;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("issuance worker did not commit")
    }
    async fn send(
        &self,
        method: &str,
        path: &str,
        cookie: &str,
        origin: &str,
        body: &str,
    ) -> Response {
        send(&self.app, method, path, cookie, origin, body).await
    }
    async fn cleanup(self) {
        self.worker.abort();
        let _ = self.worker.await;
        sql(&self.url,&format!("BEGIN;SELECT live_proofs FROM flow_observation_state WHERE singleton FOR UPDATE;WITH deleted AS (DELETE FROM flow_observation_proofs WHERE book_id={} RETURNING 1) UPDATE flow_observation_state SET live_proofs=live_proofs-(SELECT count(*) FROM deleted) WHERE singleton;DELETE FROM flow_observation_hourly WHERE book_id={};DELETE FROM books WHERE id={};DELETE FROM account_roles WHERE user_id={};DELETE FROM users WHERE id={};COMMIT",self.book,self.book,self.book,self.actor,self.actor));
    }
}
async fn send(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    origin: &str,
    body: &str,
) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, origin)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-tor-observation", "1")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}
fn payload(proof: &str, kind: &str) -> Value {
    json!({"proof":proof,"kind":kind,"visible_percent":60,"continuous_visible_ms":1000,"foreground":true})
}
fn private(response: &Response, expected: StatusCode) {
    assert_eq!(response.status(), expected);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    instrumented(response, expected);
}
fn instrumented(response: &Response, expected: StatusCode) {
    use tor_route_intelligence::{RouteObservation, StatusBucket};
    let observation = response
        .extensions()
        .get::<RouteObservation>()
        .expect("post-base merged route must execute RIL");
    assert!(
        observation.passport.route_known,
        "reviewed route must remain known even on denial"
    );
    assert_eq!(
        observation.status_bucket,
        if expected.is_success() {
            StatusBucket::Success
        } else if expected.is_client_error() {
            StatusBucket::ClientError
        } else {
            StatusBucket::ServerError
        }
    );
    let id = response
        .headers()
        .get("x-request-id")
        .expect("server-issued trace ID")
        .to_str()
        .unwrap();
    assert_eq!(id.len(), 32);
    assert!(id.bytes().all(|c| c.is_ascii_hexdigit()));
    let trace = response
        .extensions()
        .get::<tor_observability::services::trace::RequestTrace>()
        .expect("correlated numerical trace");
    assert_eq!(format!("{:032x}", trace.trace_id), id);
}
async fn text(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
fn sql(url: &str, statement: &str) -> String {
    let result = Command::new("psql")
        .args([
            "-XAtq",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            url,
            "-c",
            statement,
        ])
        .env(
            "PGOPTIONS",
            "-c statement_timeout=5000 -c lock_timeout=2000",
        )
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().into()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn anonymous_ingest_enforces_origin_shape_timing_and_replay_without_cookie_or_identity() {
    let f = Fixture::new().await;
    let proof = f.proof().await;
    let body = payload(&proof, "rendered").to_string();
    private(
        &f.send("POST", API, "", "https://evil.invalid", &body).await,
        StatusCode::FORBIDDEN,
    );
    let no_header = f
        .app
        .clone()
        .oneshot(
            Request::post(API)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::ORIGIN, ORIGIN)
                .body(Body::from(body.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    private(&no_header, StatusCode::FORBIDDEN);
    private(
        &f.send("POST", API, "", ORIGIN, &body).await,
        StatusCode::NO_CONTENT,
    );
    private(
        &f.send("POST", API, "", ORIGIN, &body).await,
        StatusCode::NO_CONTENT,
    );
    let mut forged = payload(&proof, "view");
    forged["book_id"] = json!(f.book);
    private(
        &f.send("POST", API, "", ORIGIN, &forged.to_string()).await,
        StatusCode::UNPROCESSABLE_ENTITY,
    );
    private(
        &f.send("POST", API, "", ORIGIN, &"x".repeat(5000)).await,
        StatusCode::PAYLOAD_TOO_LARGE,
    );
    let mut below = payload(&proof, "view");
    below["continuous_visible_ms"] = json!(999);
    private(
        &f.send("POST", API, "", ORIGIN, &below.to_string()).await,
        StatusCode::BAD_REQUEST,
    );
    tokio::time::sleep(Duration::from_millis(1100)).await;
    private(
        &f.send(
            "POST",
            API,
            "",
            ORIGIN,
            &payload(&proof, "view").to_string(),
        )
        .await,
        StatusCode::NO_CONTENT,
    );
    private(
        &f.send(
            "POST",
            API,
            "",
            ORIGIN,
            &payload(&proof, "click").to_string(),
        )
        .await,
        StatusCode::NO_CONTENT,
    );
    assert_eq!(sql(&f.url,&format!("SELECT opportunities||','||rendered||','||views||','||clicks||','||replayed FROM flow_observation_hourly WHERE book_id={}",f.book)),"1,1,1,1,1");
    sql(
        &f.url,
        &format!("UPDATE books SET admin_hold=true WHERE id={}", f.book),
    );
    private(
        &f.send("POST", API, "", ORIGIN, &body).await,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        sql(
            &f.url,
            &format!(
                "SELECT count(*) FROM rec_subjects WHERE user_id={}",
                f.actor
            )
        ),
        "0"
    );
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn administrator_report_uses_fresh_roles_and_ssr_proofs_are_from_trusted_renderers() {
    let f = Fixture::new().await;
    for path in ["/", "/catalog"] {
        let head = f.send("HEAD", path, "", ORIGIN, "").await;
        assert_eq!(head.status(), StatusCode::OK);
        instrumented(&head, StatusCode::OK);
        assert!(text(head).await.is_empty());
        assert_eq!(
            f.runtime.queue_snapshot().queued,
            0,
            "HEAD must not issue card proofs"
        );
    }
    for method in ["GET", "HEAD"] {
        let asset = f
            .send(method, "/assets/flow-observations.js", "", ORIGIN, "")
            .await;
        assert_eq!(asset.status(), StatusCode::OK);
        instrumented(&asset, StatusCode::OK);
        assert_eq!(
            asset
                .extensions()
                .get::<tor_route_intelligence::RouteObservation>()
                .unwrap()
                .passport
                .route_family,
            "/assets/"
        );
        if method == "HEAD" {
            assert!(text(asset).await.is_empty());
        }
        let denied = f.send(method, REPORT, "", ORIGIN, "").await;
        private(&denied, StatusCode::UNAUTHORIZED);
        assert_eq!(
            denied
                .extensions()
                .get::<tor_route_intelligence::RouteObservation>()
                .unwrap()
                .passport
                .route_family,
            "/admin/"
        );
        if method == "HEAD" {
            assert!(text(denied).await.is_empty());
        }
    }
    private(
        &f.send("GET", REPORT, "", ORIGIN, "").await,
        StatusCode::UNAUTHORIZED,
    );
    private(
        &f.send("GET", &format!("{REPORT}?reader=99"), &f.cookie, ORIGIN, "")
            .await,
        StatusCode::FORBIDDEN,
    );
    sql(
        &f.url,
        &format!(
            "INSERT INTO account_roles(user_id,role) VALUES({},'administrator')",
            f.actor
        ),
    );
    let page = f.send("GET", REPORT, &f.cookie, ORIGIN, "").await;
    private(&page, StatusCode::OK);
    assert_eq!(page.headers()["x-robots-tag"], "noindex, nofollow");
    let html = text(page).await;
    assert!(html.contains("неизвестно"));
    assert!(!html.contains("<script>safe"));
    assert!(!html.contains("data-flow-proof"));
    private(
        &f.send("GET", &format!("{REPORT}?days=91"), &f.cookie, ORIGIN, "")
            .await,
        StatusCode::BAD_REQUEST,
    );
    let home = text(f.send("GET", "/", "", ORIGIN, "").await).await;
    assert!(home.contains("data-flow-proof=\""));
    assert!(home.contains("/assets/flow-observations.js"));
    assert!(!home.contains("<script>safe"));
    let catalogue = text(f.send("GET", "/catalog", "", ORIGIN, "").await).await;
    assert!(catalogue.contains("data-flow-proof=\""));
    assert!(catalogue.contains("data-flow-link"));
    let private_library = text(f.send("GET", "/me/library", &f.cookie, ORIGIN, "").await).await;
    assert!(!private_library.contains("data-flow-proof"));
    let proof = f.proof().await;
    sql(
        &f.url,
        &format!(
            "UPDATE sessions SET revoked_at=now() WHERE user_id={}",
            f.actor
        ),
    );
    private(
        &f.send(
            "POST",
            API,
            &f.cookie,
            ORIGIN,
            &payload(&proof, "click").to_string(),
        )
        .await,
        StatusCode::FORBIDDEN,
    );
    private(
        &f.send("GET", REPORT, &f.cookie, ORIGIN, "").await,
        StatusCode::UNAUTHORIZED,
    );
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn editorial_guard_receives_only_issued_context_and_registration_queue_never_waits_for_sql() {
    let f = Fixture::new().await;
    let catalog = PgCatalog::connect(&f.url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool().clone());
    let mut card = f.card();
    card.surface = ExposureSurface::Home;
    card.editorial = Some(EditorialPlacement {
        campaign_id: 19,
        revision: 3,
        slot: PromotionSlot::HomeFeatured,
    });
    let guard = Arc::new(Guard {
        allowed: AtomicBool::new(true),
        expected: card,
    });
    let (runtime, mut worker) =
        Runtime::new(accounts.clone(), ORIGIN.into(), false, Some(guard.clone()));
    worker.prepare().await.unwrap();
    let issuer = runtime.issuer();
    let app = runtime.attach(Router::new());
    let handle = tokio::spawn(worker.run());
    sql(
        &f.url,
        &format!(
            "INSERT INTO account_roles(user_id,role) VALUES({},'administrator')",
            f.actor
        ),
    );
    let campaign = send(
        &app,
        "GET",
        "/admin/audience/exposure?source=editorial&campaign=19",
        &f.cookie,
        ORIGIN,
        "",
    )
    .await;
    private(&campaign, StatusCode::OK);
    let html = text(campaign).await;
    assert!(html.contains(&format!("data-book-id=\"{}\"", f.book)));
    assert!(html.contains("data-metric=\"books\">1</span>"));
    assert!(html.contains("data-metric=\"views\">неизвестно</dd>"));
    private(
        &send(
            &app,
            "GET",
            "/admin/audience/exposure?source=editorial&campaign=20",
            &f.cookie,
            ORIGIN,
            "",
        )
        .await,
        StatusCode::NOT_FOUND,
    );
    let proof = issuer.issue(card).unwrap();
    for _ in 0..100 {
        if f.store.describe(&proof).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    private(
        &send(
            &app,
            "POST",
            API,
            "",
            ORIGIN,
            &payload(&proof, "click").to_string(),
        )
        .await,
        StatusCode::NO_CONTENT,
    );
    guard.allowed.store(false, Ordering::Relaxed);
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
        StatusCode::FORBIDDEN,
    );
    handle.abort();
    let _ = handle.await;
    // Before registration there is no proof; after prepare, a parked consumer
    // admits only the finite queue while its monotonic/database lease is live.
    let (paused, mut parked) = Runtime::new(accounts, ORIGIN.into(), false, None);
    let issuer = paused.issuer();
    assert!(issuer.issue(f.card()).is_none());
    assert_eq!(paused.queue_snapshot().queued, 0);
    parked.prepare().await.unwrap();
    let start = std::time::Instant::now();
    for _ in 0..1024 {
        assert!(issuer.issue(f.card()).is_some());
    }
    assert!(issuer.issue(f.card()).is_none());
    assert!(start.elapsed() < Duration::from_secs(1));
    assert_eq!(paused.queue_snapshot().dropped, 2);
    f.cleanup().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn authority_lock_fails_closed_and_releases_the_production_reader_pool() {
    use std::io::{BufRead, BufReader, Write};
    use std::process::{Child, ChildStdin, Stdio};

    struct HeldLock(Child, Option<ChildStdin>);
    impl Drop for HeldLock {
        fn drop(&mut self) {
            if let Some(mut input) = self.1.take() {
                let _ = writeln!(input, "ROLLBACK;\n\\q");
                let _ = input.flush();
            }
            let _ = self.0.wait();
        }
    }

    // Fixture uses PgCatalog::connect's real three-connection pool shared by
    // A, F and the reader. A cancelled HTTP future must not leave SQL running.
    let f = Fixture::new().await;
    let proof = f.proof().await;
    let mut child = Command::new("psql")
        .args(["-XAtq", "-v", "ON_ERROR_STOP=1", "--dbname", &f.url])
        .env(
            "PGOPTIONS",
            "-c statement_timeout=5000 -c idle_in_transaction_session_timeout=10000",
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    let mut output = BufReader::new(child.stdout.take().unwrap());
    writeln!(
        input,
        "BEGIN; LOCK TABLE users IN ACCESS EXCLUSIVE MODE; SELECT 'READY';"
    )
    .unwrap();
    input.flush().unwrap();
    loop {
        let mut line = String::new();
        assert!(output.read_line(&mut line).unwrap() > 0);
        if line.trim() == "READY" {
            break;
        }
    }
    let held = HeldLock(child, Some(input));
    let body = payload(&proof, "click").to_string();
    let started = std::time::Instant::now();
    let responses = tokio::join!(
        f.send("POST", API, &f.cookie, ORIGIN, &body),
        f.send("POST", API, &f.cookie, ORIGIN, &body),
        f.send("POST", API, &f.cookie, ORIGIN, &body)
    );
    let elapsed = started.elapsed();
    for response in [&responses.0, &responses.1, &responses.2] {
        private(response, StatusCode::SERVICE_UNAVAILABLE);
    }
    assert!(elapsed < Duration::from_millis(900));
    let waiting = sql(&f.url, "SELECT count(*) FROM pg_stat_activity WHERE datname=current_database() AND usename='tor_app' AND wait_event_type='Lock' AND query LIKE '%SELECT u.id%'");
    assert_eq!(waiting, "0", "authority SQL must stop before HTTP returns");
    // This lookup uses the same pool but no users table. It must work while
    // the exclusive users lock is STILL HELD, not only after its release.
    tokio::time::timeout(Duration::from_millis(300), f.store.describe(&proof))
        .await
        .expect("shared pool retained cancelled authority work")
        .expect("independent reader query should remain available");
    assert_eq!(
        sql(
            &f.url,
            &format!(
                "SELECT coalesce(sum(clicks),0) FROM flow_observation_hourly WHERE book_id={}",
                f.book
            )
        ),
        "0"
    );
    println!("authority lock: three 503 responses in {} ms, zero blocked authority SQL, shared pool reusable before unlock", elapsed.as_millis());
    assert!(!f.runtime.queue_snapshot().ready);
    drop(held);
    // Reader recovery is immediate; optional observation issuance waits for the
    // next bounded durable-health acknowledgement before reopening its gate.
    assert_eq!(
        f.send("GET", "/catalog", "", ORIGIN, "").await.status(),
        StatusCode::OK
    );
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    while !f.runtime.queue_snapshot().ready {
        assert!(tokio::time::Instant::now() < deadline, "collector did not re-acknowledge its healthy source");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(
        f.send("GET", "/catalog", "", ORIGIN, "").await.status(),
        StatusCode::OK
    );
    private(
        &f.send("POST", API, &f.cookie, ORIGIN, &body).await,
        StatusCode::NO_CONTENT,
    );
    f.cleanup().await;
}

#[path = "exposure_http_pg/health.rs"]
mod health;
