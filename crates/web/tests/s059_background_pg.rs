//! Real HTTP, PostgreSQL queue, worker processes and actual SQLite writer.
//! The private source-store fixture injects faults; this is not an S3 drill.
#![cfg(unix)]

#[path = "s059_background_pg/support.rs"]
mod support;

use reqwest::{Client, StatusCode};
use serde_json::{json, Value};
use std::{
    fs,
    num::NonZeroU32,
    os::unix::fs::PermissionsExt,
    process::Command,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use support::{children, guard, rejected_sql, sql, worker, writer, FixtureStore, DATABASE};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::{
    repositories::{postgres::PgAuthoring, PgCatalog},
    services::{
        authoring::new_book,
        chapter_file_import::{fingerprint, parse_text},
    },
};
use tor_observability::services::{
    lookup,
    trace::{background::{lookup::BackgroundPage, Outcome}, RequestTrace},
};

const TEXT: &str = "Глава 1: Проверка\nS059_PRIVATE_IMPORT_BODY_A\n\nГлава 2: Завершение\nS059_PRIVATE_IMPORT_BODY_B";
const FILENAME: &str = "s059-private-source.txt";

struct Server(tokio::task::JoinHandle<()>);
impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct Web {
    client: Client,
    base: String,
    cookie: String,
    stranger: String,
    csrf: String,
    password: String,
}

struct Queued {
    book: i64,
    job: i64,
    parent: u128,
    payload: Value,
}

impl Web {
    async fn post(&self, path: &str, cookie: &str, body: &Value) -> reqwest::Response {
        self.client
            .post(format!("{}{path}", self.base))
            .header("origin", &self.base)
            .header("cookie", cookie)
            .header("x-request-id", "ffffffffffffffffffffffffffffffff")
            .json(body)
            .send()
            .await
            .unwrap()
    }

    async fn enqueue(&self, authoring: &PgAuthoring, owner: i64, label: &str) -> Queued {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let book = new_book(
            &format!("tor-ci:s059-background:{label}:{nonce}"),
            "ru",
            "Private background fixture",
            &generate_session_token(),
        )
        .unwrap();
        let book = authoring.create_owned_book(owner, &book).await.unwrap();
        let chapters = parse_text(FILENAME, TEXT).unwrap();
        let payload = json!({
            "filename": FILENAME, "text": TEXT, "csrf_token": self.csrf,
            "command_key": generate_session_token(),
            "fingerprint": hex::encode(fingerprint(book, &chapters)),
            "origin_trace_id": "ffffffffffffffffffffffffffffffff",
        });
        let path = format!("/me/books/{book}/chapters/import-queue");
        if label == "retry" {
            assert_eq!(self.post(&path, "", &payload).await.status(), StatusCode::FORBIDDEN);
            let mut bad = payload.clone();
            bad["csrf_token"] = json!("invalid");
            assert_eq!(self.post(&path, &self.cookie, &bad).await.status(), StatusCode::FORBIDDEN);
            assert_eq!(self.post(&path, &self.stranger, &payload).await.status(), StatusCode::NOT_FOUND);
            assert_eq!(sql(&format!("SELECT count(*) FROM chapter_import_jobs WHERE book_id={book}")), "0");
        }
        let response = self.post(&path, &self.cookie, &payload).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let trace = response.headers()["x-request-id"].to_str().unwrap();
        assert_eq!(trace.len(), 32);
        let parent = u128::from_str_radix(trace, 16).unwrap();
        assert_ne!(parent, u128::MAX);
        let value: Value = response.json().await.unwrap();
        assert!(value.get("origin_trace_id").is_none());
        let job = value["job_id"].as_i64().unwrap();
        assert!(job > 0);
        assert_eq!(origin(job), format!("{parent:032x}"));
        Queued { book, job, parent, payload }
    }

    async fn operator(&self, parent: u128) -> String {
        let response = self.client
            .get(format!("{}/ops/traces?id={parent:032x}", self.base))
            .basic_auth("operator", Some(&self.password))
            .send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        assert_eq!(response.headers()["x-frame-options"], "DENY");
        let text = response.text().await.unwrap();
        for forbidden in [FILENAME, "S059_PRIVATE_IMPORT_BODY", "s059-private-owner", "staging/chapters/"] {
            assert!(!text.contains(forbidden), "private import data escaped into diagnostics");
        }
        text
    }
}

fn origin(job: i64) -> String {
    sql(&format!("SELECT encode(origin_trace_id,'hex') FROM chapter_import_jobs WHERE id={job}"))
}

fn count_chapters(book: i64) -> String {
    sql(&format!("SELECT count(*) FROM chapters WHERE book_id={book}"))
}

async fn root(parent: u128) -> RequestTrace {
    let socket = guard().join("private/background.sock");
    let started = Instant::now();
    loop {
        if let Ok(found) = lookup::request(&socket, parent) {
            if let Some((_, root)) = found.found {
                return root;
            }
        }
        assert!(started.elapsed() < Duration::from_secs(10), "HTTP root missing");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

async fn wait_writer() {
    let started = Instant::now();
    while children(0).is_none() {
        assert!(started.elapsed() < Duration::from_secs(10), "actual writer unavailable");
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
}

fn assert_outcomes(parent: u128, expected: &[(u8, Outcome)]) -> BackgroundPage {
    let page = children(parent).expect("actual writer lookup");
    let mut actual: Vec<_> = page.items.iter().map(|(_, r)| (r.attempt, r.outcome)).collect();
    actual.sort_by_key(|row| row.0);
    assert_eq!(actual, expected);
    assert_eq!(page.matches as usize, expected.len());
    for (_, row) in &page.items {
        assert_eq!(row.parent_trace_id, parent);
        assert_ne!(row.trace_id, parent);
        assert!(row.encode().is_some());
    }
    page
}

fn expire(job: i64) {
    sql(&format!("UPDATE chapter_import_jobs SET run_after=now()-INTERVAL '1 second', locked_until=CASE WHEN status='running' THEN now()-INTERVAL '1 second' ELSE locked_until END WHERE id={job}"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "run by the existing S059 reader-browser CI step on disposable tor_ci"]
async fn s059_http_import_retry_and_restart_evidence() {
    let output = guard();
    let source_dir = output.join("private/background-store");
    fs::create_dir(&source_dir).unwrap();
    fs::set_permissions(&source_dir, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(sql("SELECT count(*) FROM chapter_import_jobs WHERE status IN ('queued','running')"), "0");
    assert!(tor_web::activate_ril_sqlx_observation());
    let catalog = PgCatalog::connect(DATABASE).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool());
    let authoring = PgAuthoring::new(catalog.pool());
    let token = generate_session_token();
    let stranger = generate_session_token();
    let csrf = generate_session_token();
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let password_hash = hash_password("S059DisposableFixtureOnly2026").unwrap();
    let owner = accounts.register_with_session(
        &format!("s059-private-owner-{nonce}@example.invalid"), "Trace owner", &password_hash,
        &session_token_digest(&token), NonZeroU32::new(3600).unwrap(),
    ).await.unwrap();
    accounts.register_with_session(
        &format!("s059-stranger-{nonce}@example.invalid"), "Trace stranger", &password_hash,
        &session_token_digest(&stranger), NonZeroU32::new(3600).unwrap(),
    ).await.unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = tor_web::router_with_postgres(catalog.clone()).merge(
        tor_web::instrument_additional_routes(tor_web::chapter_import_queue_http::router(
            accounts, false, base.clone(), Arc::new(FixtureStore::new("http")),
        )),
    );
    let _server = Server(tokio::spawn(async move { axum::serve(listener, app).await.unwrap(); }));
    let web = Web {
        client: Client::builder().timeout(Duration::from_secs(8)).build().unwrap(),
        base,
        cookie: format!("tor-dev-session={token}; tor-dev-csrf={csrf}"),
        stranger: format!("tor-dev-session={stranger}; tor-dev-csrf={csrf}"),
        csrf,
        password: std::env::var("TOR_OBS_OPERATOR_PASSWORD").unwrap(),
    };
    let mut store = writer();
    wait_writer().await;

    let retry = web.enqueue(&authoring, owner, "retry").await;
    let http_before = root(retry.parent).await;
    assert!(http_before.sqlx.as_ref().is_some_and(|sql| sql.observed_queries > 0));
    let replay = web.post(&format!("/me/books/{}/chapters/import-queue", retry.book), &web.cookie, &retry.payload).await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert_ne!(replay.headers()["x-request-id"], format!("{:032x}", retry.parent));
    assert_eq!(replay.json::<Value>().await.unwrap()["job_id"], retry.job);
    assert_eq!(origin(retry.job), format!("{:032x}", retry.parent));
    for value in ["NULL", "decode(repeat('ab',16),'hex')"] {
        rejected_sql(&format!("UPDATE chapter_import_jobs SET origin_trace_id={value} WHERE id={}", retry.job), "trace origin is immutable");
    }
    sql(&format!("UPDATE chapter_import_jobs SET origin_trace_id=origin_trace_id WHERE id={}", retry.job));
    let mut process = worker("retry", retry.parent, 1);
    process.wait().await;
    assert_outcomes(retry.parent, &[(1, Outcome::Retried)]);
    assert_eq!(count_chapters(retry.book), "0");
    store.kill();
    store = writer();
    wait_writer().await;
    assert_outcomes(retry.parent, &[(1, Outcome::Retried)]);
    expire(retry.job);
    worker("complete", retry.parent, 2).wait().await;
    let attempts = assert_outcomes(retry.parent, &[(1, Outcome::Retried), (2, Outcome::Completed)]);
    assert_eq!(count_chapters(retry.book), "2");
    assert_eq!(root(retry.parent).await, http_before, "worker time must not rewrite HTTP duration");
    assert!(attempts.items.iter().all(|(_, r)| r.phase_us[0] >= 200_000));

    let cancelled = web.enqueue(&authoring, owner, "cancelled").await;
    root(cancelled.parent).await;
    let mut blocked = worker("gated", cancelled.parent, 1);
    support::wait_entered().await;
    let mut catalog_ms = Vec::new();
    for _ in 0..20 {
        let started = Instant::now();
        let response = web.client.get(format!("{}/api/v1/books", web.base)).send().await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.json::<Value>().await.unwrap().is_array());
        catalog_ms.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    let cancelled_response = web.post(
        &format!("/me/books/{}/chapters/import-jobs/{}/cancel", cancelled.book, cancelled.job),
        &web.cookie, &json!({"csrf_token": web.csrf}),
    ).await;
    assert_eq!(cancelled_response.status(), StatusCode::OK);
    assert_eq!(cancelled_response.json::<Value>().await.unwrap()["cancelled"], true);
    fs::write(source_dir.join("release"), b"go").unwrap();
    blocked.wait().await;
    assert_outcomes(cancelled.parent, &[(1, Outcome::Stale)]);
    assert_eq!(count_chapters(cancelled.book), "0");
    assert_eq!(origin(cancelled.job), format!("{:032x}", cancelled.parent));

    let interrupted = web.enqueue(&authoring, owner, "interrupted").await;
    root(interrupted.parent).await;
    worker("interrupt", interrupted.parent, 1).wait().await;
    assert_outcomes(interrupted.parent, &[(1, Outcome::Interrupted)]);
    expire(interrupted.job);
    worker("complete", interrupted.parent, 2).wait().await;
    assert_outcomes(interrupted.parent, &[(1, Outcome::Interrupted), (2, Outcome::Completed)]);
    assert_eq!(count_chapters(interrupted.book), "2");

    let killed = web.enqueue(&authoring, owner, "killed").await;
    root(killed.parent).await;
    let mut lost = worker("gated", killed.parent, 1);
    support::wait_entered().await;
    lost.kill();
    assert_outcomes(killed.parent, &[]);
    expire(killed.job);
    worker("complete", killed.parent, 2).wait().await;
    assert_outcomes(killed.parent, &[(2, Outcome::Completed)]);
    assert_eq!(count_chapters(killed.book), "2");

    let corrupt = web.enqueue(&authoring, owner, "corrupt").await;
    root(corrupt.parent).await;
    worker("corrupt", corrupt.parent, 1).wait().await;
    assert_outcomes(corrupt.parent, &[(1, Outcome::Failed)]);
    assert_eq!(count_chapters(corrupt.book), "0");

    let outage = web.enqueue(&authoring, owner, "outage").await;
    root(outage.parent).await;
    store.kill();
    worker("down", outage.parent, 1).wait().await;
    assert_eq!(count_chapters(outage.book), "2");
    assert_eq!(sql(&format!("SELECT status FROM chapter_import_jobs WHERE id={}", outage.job)), "succeeded");
    store = writer();
    wait_writer().await;
    assert_outcomes(outage.parent, &[]);
    assert!(web.operator(outage.parent).await.contains("не доказывает"));

    for headers in [None, Some(&web.cookie)] {
        let mut request = web.client.get(format!("{}/ops/traces?id={:032x}", web.base, retry.parent));
        if let Some(cookie) = headers { request = request.header("cookie", cookie); }
        let denied = request.send().await.unwrap();
        assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(denied.headers()["cache-control"], "no-store");
    }
    let html = web.operator(retry.parent).await;
    assert!(html.contains("data-background-matches=\"2\""));
    assert!(html.contains("data-background-outcome=\"retried\""));
    assert!(html.contains("data-background-outcome=\"completed\""));
    let cases = json!([
        {"trace": format!("{:032x}", retry.parent), "outcomes": ["retried","completed"]},
        {"trace": format!("{:032x}", interrupted.parent), "outcomes": ["interrupted","completed"]},
        {"trace": format!("{:032x}", killed.parent), "outcomes": ["completed"]}
    ]);
    let origin = web.base.clone();
    let password = web.password.clone();
    let destination = output.clone();
    let browser_cases = cases.to_string();
    let browser = tokio::task::spawn_blocking(move || {
        Command::new("node").arg("tools/operator_trace_background_browser.cjs")
            .env("TOR_S059_ORIGIN", origin)
            .env("TOR_TEST_OPERATOR_PASSWORD", password)
            .env("TOR_S059_OUTPUT", destination)
            .env("TOR_S059_BACKGROUND_CASES", browser_cases)
            .status().unwrap()
    }).await.unwrap();
    assert!(browser.success(), "actual background operator browser checks failed");
    catalog_ms.sort_by(f64::total_cmp);
    let p95 = catalog_ms[18];
    assert!(p95 < 2000.0, "debug CI catalog budget while import was gated");
    let bytes = fs::read(output.join("private/background.sqlite3")).unwrap();
    for secret in [FILENAME, "S059_PRIVATE_IMPORT_BODY", "s059-private-owner", "staging/chapters/"] {
        assert!(!bytes.windows(secret.len()).any(|part| part == secret.as_bytes()));
    }
    let report = json!({
        "scenario": "S059_background_import", "source_sha": std::env::var("GITHUB_SHA").unwrap(),
        "real_http": true, "real_postgresql": true, "real_writer_process": true,
        "separate_worker_processes": true, "source_store": "fault_controlled_private_fixture_not_s3",
        "client_trace_rejected": true, "replay_origin_preserved": true,
        "origin_update_rejected": true, "http_duration_not_extended": true,
        "retry_then_completed": true, "writer_restart_preserved_attempt": true,
        "cancelled_worker_stale": true, "cooperative_interruption_recorded": true,
        "killed_worker_missing_receipt_not_fabricated": true,
        "lease_reclaim_kept_origin": true, "corrupt_source_rejected": true,
        "writer_outage_did_not_fail_import": true, "private_operator_browser": true,
        "catalog_samples_while_worker_gated": 20, "catalog_p95_ms": p95,
        "debug_ci_budget_ms": 2000, "cases": cases,
        "other_background_workers_tested": false, "production_slo_proven": false,
        "independent_staging_tested": false, "release_approved": false
    });
    fs::write(output.join("background-report.json"), serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    println!("S059_BACKGROUND_REAL_HTTP_PG_WRITER_BROWSER_OK");
    store.kill();
}
