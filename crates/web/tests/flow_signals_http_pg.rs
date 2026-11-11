//! Actual PG authorization, durable episode rendering, read-only GET and bounded failure.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use std::{
    io::{BufRead, BufReader, Write},
    num::NonZeroU32,
    process::{Child, ChildStdout, Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tor_recommendations::{
    repositories::flow_signals::{PgFlowSignals, SignalObservation},
    services::flow_signals::{source_scope, EventKind, Evidence, Rule, SignalWindow, Status},
};
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3197";
fn disposable(url: &str) {
    let parsed = reqwest::Url::parse(url).expect("disposable PostgreSQL URL");
    let port: u16 = std::env::var("FLOW_TEST_PG_PORT").unwrap_or_else(|_| "5432".into()).parse().expect("port");
    assert!(port > 0);
    assert!(matches!(parsed.scheme(), "postgres" | "postgresql"));
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert_eq!(parsed.port(), Some(port));
    assert_eq!(parsed.path(), "/tor_ci");
    assert!(parsed.query().is_none() && parsed.fragment().is_none());
}
fn pg(url: &str, sql: &str) -> String {
    disposable(url);
    let out = Command::new("psql").env("PGOPTIONS", "-c statement_timeout=5000 -c lock_timeout=2000")
        .args(["-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1", "--dbname", url, "--command", sql])
        .output().expect("psql");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().into()
}
struct HeldLock {
    child: Child,
    _output: BufReader<ChildStdout>,
}
impl HeldLock {
    fn new(url: &str) -> Self {
        disposable(url);
        let mut child = Command::new("psql")
            .args(["-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1", "--dbname", url])
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit()).spawn().unwrap();
        writeln!(child.stdin.as_mut().unwrap(), "BEGIN; SET LOCAL lock_timeout='2s'; LOCK TABLE flow_signal_states IN ACCESS EXCLUSIVE MODE; SELECT 'FLOW_LOCKED';").unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        output.read_line(&mut line).unwrap();
        assert_eq!(line.trim(), "FLOW_LOCKED");
        Self { child, _output: output }
    }
    fn release(mut self) {
        writeln!(self.child.stdin.as_mut().unwrap(), "ROLLBACK;\n\\q").unwrap();
        assert!(self.child.wait().unwrap().success());
    }
}
impl Drop for HeldLock {
    fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); }
}
async fn get(app: &Router, path: &str, cookie: &str, expected: StatusCode) -> String {
    let response = app.clone().oneshot(Request::builder().uri(path)
        .header(header::COOKIE, cookie).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(response.status(), expected, "{path}");
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow");
    assert!(response.headers()["content-security-policy"].to_str().unwrap().contains("script-src 'none'"));
    String::from_utf8(to_bytes(response.into_body(), 256 * 1024).await.unwrap().to_vec()).unwrap()
}
fn fingerprint(url: &str) -> String {
    pg(url, "SELECT (SELECT count(*) FROM flow_signal_events),(SELECT count(*) FROM flow_signal_states),(SELECT count(*) FROM flow_public_page_results),(SELECT count(*) FROM rec_subjects),(SELECT count(*) FROM outbox_events)")
}
async fn round(journal: &PgFlowSignals, seed: &str, evidence: Evidence) -> String {
    let observations = Rule::ALL.into_iter().filter(|rule| rule.accepts_days(7)).map(|rule| SignalObservation {
        rule, scope: source_scope(&["signal-http-fixture", seed, rule.code()]),
        window: SignalWindow { until_epoch: Some(3_600), label: "fixture <script>bad()</script>".into() },
        evidence: if rule == Rule::HtmlQuality { evidence } else { Evidence::missing(true) },
    }).collect::<Vec<_>>();
    let scope = observations.iter().find(|o| o.rule == Rule::HtmlQuality).unwrap().scope.clone();
    let lease = journal.claim_round(7).await.unwrap().expect("uncontended test lease");
    journal.record_round(lease, &observations).await.unwrap();
    scope
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci with both app and migrator URLs"]
async fn signals_and_pages_have_live_authority_saved_evidence_and_bounded_read_only_get() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test app URL");
    let migrator = std::env::var("MIGRATOR_URL").expect("test migrator URL");
    disposable(&url); disposable(&migrator);
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos().to_string();
    let password = hash_password("FlowSignalsHttpFixture2026").unwrap();
    let mut users = Vec::new();
    let mut cookies = Vec::new();
    let mut tokens = Vec::new();
    for kind in ["administrator", "reader"] {
        let token = generate_session_token();
        let user = accounts.register_with_session(&format!("signal-{kind}-{stamp}@example.invalid"),
            "Signal fixture", &password, &session_token_digest(&token), NonZeroU32::new(3600).unwrap()).await.unwrap();
        users.push(user); cookies.push(format!("tor-dev-session={token}")); tokens.push(token);
    }
    pg(&url, &format!("INSERT INTO account_roles(user_id,role) VALUES({},'administrator')", users[0]));
    let app = tor_web::router_with_postgres_reading_auth(catalog, PgReader::new(pool.clone()),
        accounts.clone(), false, ORIGIN.into());
    for path in ["/admin/audience/signals?days=bad", "/admin/audience/pages?unknown=true"] {
        get(&app, path, "", StatusCode::UNAUTHORIZED).await;
        get(&app, path, &cookies[1], StatusCode::FORBIDDEN).await;
        get(&app, path, &cookies[0], StatusCode::BAD_REQUEST).await;
    }
    for query in ["days=0", "days=2", "days=7&days=1", "before=0", "before=-1", "unknown=true"] {
        get(&app, &format!("/admin/audience/signals?{query}"), &cookies[0], StatusCode::BAD_REQUEST).await;
    }
    let journal = PgFlowSignals::new(pool.clone());
    let mut known = Evidence::missing(true);
    known.available = true;
    known.expected_units = 168;
    known.current_units = 168;
    known.previous_units = 168;
    known.current = Some(100);
    known.previous = Some(100);
    known.known_discards = 1;
    let scope = round(&journal, &stamp, known).await;
    round(&journal, &stamp, Evidence::missing(true)).await;
    let state = journal.report(7, None).await.unwrap();
    let current = state.states.iter().find(|state| state.scope == scope).unwrap();
    assert_eq!(current.result.status, Status::Unknown);
    assert!(current.episode.reason.is_some());
    assert_eq!(state.events.iter().filter(|event| event.state.scope == scope && event.kind == EventKind::Opened).count(), 1);
    assert!(!state.events.iter().any(|event| event.state.scope == scope && event.kind == EventKind::Recovered));
    let before = fingerprint(&url);
    for days in [1, 7, 30, 90] {
        let html = get(&app, &format!("/admin/audience/signals?days={days}"), &cookies[0], StatusCode::OK).await;
        assert_eq!(html.matches(r#"data-testid="signal-state""#).count(), 13);
        assert!(!html.contains(&format!("signal-administrator-{stamp}")));
        if days == 7 {
            assert!(html.contains("signal-open-episode"));
            assert!(html.contains("&lt;script&gt;bad()&lt;/script&gt;"));
            assert!(!html.contains("<script>bad()"));
        }
    }
    let pages = get(&app, "/admin/audience/pages", &cookies[0], StatusCode::OK).await;
    assert_eq!(pages.matches(r#"data-testid="public-page-state""#).count(), 4);
    assert!(!pages.contains("pages-current-healthy"));
    assert_eq!(before, fingerprint(&url), "GET must not write any source, journal or reader fact");

    let locked = HeldLock::new(&migrator);
    let started = Instant::now();
    let unavailable = get(&app, "/admin/audience/signals?days=7", &cookies[0], StatusCode::SERVICE_UNAVAILABLE).await;
    assert!(started.elapsed() < Duration::from_millis(1400));
    assert!(unavailable.contains("Журнал временно недоступен"));
    assert!(tokio::time::timeout(Duration::from_millis(950),
        accounts.session_actor(&session_token_digest(&tokens[0]))).await.unwrap().unwrap().is_some(),
        "a timed-out aggregate must not retain the reader pool");
    locked.release();
    known.known_discards = 0;
    round(&journal, &stamp, known).await;
    let recovered = journal.report(7, None).await.unwrap();
    assert_eq!(recovered.events.iter().filter(|event| event.state.scope == scope && event.kind == EventKind::Recovered).count(), 1);
    let html = get(&app, "/admin/audience/signals?days=7", &cookies[0], StatusCode::OK).await;
    assert!(html.contains("Подтверждено восстановление"));

    let worker = tor_web::flow_signal_worker::SignalWorker::new(&accounts, Default::default());
    assert!(worker.run_once(1).await.unwrap());
    assert!(worker.pages_once(None).await.unwrap());
    let observed = journal.report(1, None).await.unwrap();
    for rule in [Rule::GoogleQuality, Rule::GoogleClicks, Rule::YandexQuality, Rule::YandexClicks,
        Rule::ExposureQuality, Rule::ExposureChange, Rule::UnderexposedBooks, Rule::UnderexposedGenres] {
        assert_eq!(observed.states.iter().find(|state| state.rule == rule).unwrap().result.status, Status::Disabled);
    }
    assert_eq!(observed.states.iter().find(|state| state.rule == Rule::PublicPageLoss).unwrap().result.status, Status::Unknown);
    assert!(!observed.events.iter().any(|event| event.state.scope == source_scope(&["public-page-head-sample", "unconfigured"])));

    pg(&url, &format!("DELETE FROM account_roles WHERE user_id={}", users[0]));
    for path in ["/admin/audience/signals", "/admin/audience/pages"] {
        get(&app, path, &cookies[0], StatusCode::FORBIDDEN).await;
    }
    let scopes = Rule::ALL.into_iter().filter(|rule| rule.accepts_days(7))
        .map(|rule| format!("'{}'", source_scope(&["signal-http-fixture", &stamp, rule.code()])))
        .collect::<Vec<_>>().join(",");
    pg(&url, &format!("DELETE FROM flow_signal_events WHERE scope IN({scopes}); DELETE FROM flow_signal_states WHERE scope IN({scopes}); DELETE FROM users WHERE id IN({},{})", users[0], users[1]));
}
