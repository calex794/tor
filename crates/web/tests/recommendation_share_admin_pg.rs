//! REC-ADMIN-01: live Axum, PostgreSQL CAS and authorization, disposable tor_ci only.
//! PostgreSQL is accessed through psql in tests. The web crate must not depend on SQLx.
#![forbid(unsafe_code)]

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::{
    io::{BufRead, BufReader, Write},
    num::NonZeroU32,
    process::{Child, ChildStdin, Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";
const PAGE: &str = "/admin/recommendations";

fn pg(url: &str, query: &str) -> String {
    assert!(url.starts_with("postgresql://tor_app:") && url.contains("@127.0.0.1:5432/tor_ci"));
    let result = Command::new("psql")
        .env(
            "PGOPTIONS",
            "-c statement_timeout=5000 -c lock_timeout=2000",
        )
        .args(["-XAtq", "-v", "ON_ERROR_STOP=1", "--dbname", url, "-c", query])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "psql: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

// The global quota policy is a singleton shared across PostgreSQL test fixtures.
// Hold the same session-level advisory lock as the recommendation tests.
struct PolicyLock {
    child: Child,
    stdin: ChildStdin,
}
impl PolicyLock {
    fn acquire(url: &str) -> Self {
        let mut child = Command::new("psql")
            .env("PGOPTIONS", "-c statement_timeout=5000 -c lock_timeout=2000")
            .args(["-XAtq", "-v", "ON_ERROR_STOP=1", "--dbname", url])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        writeln!(
            stdin,
            "SELECT 'rec-policy-locked' FROM pg_advisory_lock(864863);"
        )
        .unwrap();
        stdin.flush().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "rec-policy-locked");
        Self { child, stdin }
    }
}
impl Drop for PolicyLock {
    fn drop(&mut self) {
        let _ = writeln!(self.stdin, "\\q");
        let _ = self.stdin.flush();
        let _ = self.child.wait();
    }
}

fn policy(url: &str) -> (i64, [i32; 5]) {
    let row = pg(
        url,
        "SELECT revision||'|'||author_pct||'|'||teammate_pct||'|'||
                 other_authors_pct||'|'||no_team_author_pct||'|'||
                 no_team_other_authors_pct
         FROM rec_shelf_share_policy WHERE singleton",
    );
    let fields: Vec<i64> = row.split('|').map(|value| value.parse().unwrap()).collect();
    assert_eq!(fields.len(), 6);
    (
        fields[0],
        [
            fields[1] as i32,
            fields[2] as i32,
            fields[3] as i32,
            fields[4] as i32,
            fields[5] as i32,
        ],
    )
}

async fn call(app: &Router, method: &str, cookie: &str, origin: &str, body: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(PAGE)
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, origin)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}

fn form(csrf: &str, revision: i64, shares: [i32; 5], action: &str) -> String {
    let mut url = reqwest::Url::parse("http://localhost/").unwrap();
    url.query_pairs_mut().extend_pairs([
        ("csrf_token", csrf.to_owned()),
        ("expected_revision", revision.to_string()),
        ("action", action.to_owned()),
        ("author_pct", shares[0].to_string()),
        ("teammate_pct", shares[1].to_string()),
        ("other_authors_pct", shares[2].to_string()),
        ("no_team_author_pct", shares[3].to_string()),
        ("no_team_other_authors_pct", shares[4].to_string()),
    ]);
    url.query().unwrap().to_owned()
}

fn assert_private(response: &Response) {
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert!(response.headers()["x-robots-tag"]
        .to_str()
        .unwrap()
        .contains("noindex"));
}

async fn body(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires isolated disposable PostgreSQL 18 tor_ci with REC-POLICY migration"]
async fn admin_shares_http_requires_live_role_csrf_and_atomic_revision() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG required");
    assert!(url.starts_with("postgresql://tor_app:") && url.contains("@127.0.0.1:5432/tor_ci"));
    let _lock = PolicyLock::acquire(&url);
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("RecommendationAdmin2026Fixture").unwrap();
    let mut users = Vec::new();
    let mut tokens = Vec::new();
    for is_admin in [true, false] {
        let token = generate_session_token();
        let id = accounts
            .register_with_session(
                &format!("rec-admin-{is_admin}-{stamp}@example.invalid"),
                "Recommendation admin fixture",
                &password,
                &session_token_digest(&token),
                NonZeroU32::new(3600).unwrap(),
            )
            .await
            .unwrap();
        if is_admin {
            pg(
                &url,
                &format!(
                    "INSERT INTO account_roles(user_id,role) VALUES({id},'administrator')"
                ),
            );
        }
        users.push(id);
        tokens.push(token);
    }
    let csrf = generate_session_token();
    let admin_cookie = format!("tor-dev-session={}; tor-dev-csrf={csrf}", tokens[0]);
    let reader_cookie = format!("tor-dev-session={}", tokens[1]);
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.into(),
    );

    let (initial_revision, previous) = policy(&url);
    let before_flow = pg(&url, "SELECT count(*) FROM flow_observation_proofs");

    for (cookie, code) in [
        ("", StatusCode::UNAUTHORIZED),
        (reader_cookie.as_str(), StatusCode::FORBIDDEN),
    ] {
        for method in ["GET", "POST"] {
            let response = call(&app, method, cookie, ORIGIN, "").await;
            assert_eq!(response.status(), code);
            assert_private(&response);
        }
    }
    let response = call(&app, "GET", &admin_cookie, ORIGIN, "").await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_private(&response);
    let html = body(response).await;
    assert!(html.contains(r#"data-testid="rec-shares-form""#));
    assert!(html.contains(&format!(r#"data-revision="{initial_revision}""#)));
    assert!(!html.contains("data-flow-proof"));

    let new_shares = [40, 30, 30, 60, 40];
    let good = form(&csrf, initial_revision, new_shares, "save");
    let bad_csrf = form("wrong", initial_revision, new_shares, "save");
    for (origin, data) in [
        (ORIGIN, bad_csrf.as_str()),
        ("https://evil.example", good.as_str()),
    ] {
        let response = call(&app, "POST", &admin_cookie, origin, data).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_private(&response);
    }
    let invalid_sum = form(&csrf, initial_revision, [50, 40, 20, 70, 30], "save");
    assert_eq!(
        call(&app, "POST", &admin_cookie, ORIGIN, &invalid_sum)
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let invalid_number = good.replace("author_pct=40", "author_pct=invalid");
    assert_eq!(
        call(&app, "POST", &admin_cookie, ORIGIN, &invalid_number)
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let response = call(&app, "POST", &admin_cookie, ORIGIN, &good).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert_private(&response);
    assert_eq!(response.headers()[header::LOCATION], PAGE);
    let (next_revision, next_policy) = policy(&url);
    // Restore before assertions to avoid contaminating other singleton fixtures.
    let restore = form(&csrf, next_revision, previous, "save");
    let restored = call(&app, "POST", &admin_cookie, ORIGIN, &restore).await;
    assert_eq!(restored.status(), StatusCode::SEE_OTHER);
    assert_eq!(next_revision, initial_revision + 1);
    assert_eq!(next_policy, new_shares);
    assert_eq!(policy(&url).1, previous);
    assert_eq!(
        call(&app, "POST", &admin_cookie, ORIGIN, &good)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let after_flow = pg(&url, "SELECT count(*) FROM flow_observation_proofs");
    assert_eq!(before_flow, after_flow);

    pg(
        &url,
        &format!(
            "DELETE FROM account_roles WHERE user_id={} AND role='administrator'",
            users[0]
        ),
    );
    pg(
        &url,
        &format!("DELETE FROM sessions WHERE user_id IN({},{})", users[0], users[1]),
    );
    pg(
        &url,
        &format!("DELETE FROM users WHERE id IN({},{})", users[0], users[1]),
    );
}
