//! FLOW-08: native admin forms, real three-slot delivery and fail-soft outages.
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
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";
const PAGE: &str = "/admin/promotion/editorial";
fn pg(url: &str, sql: &str) -> String {
    assert!(url.starts_with("postgresql://tor_app:") && url.contains("@127.0.0.1:5432/tor_ci"));
    let output = Command::new("psql")
        .env(
            "PGOPTIONS",
            "-c statement_timeout=5000 -c lock_timeout=2000",
        )
        .args(["-XAtq", "-v", "ON_ERROR_STOP=1", "--dbname", url, "-c", sql])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}
fn form(fields: &[(&str, String)]) -> String {
    let mut url = reqwest::Url::parse("http://localhost/").unwrap();
    url.query_pairs_mut()
        .extend_pairs(fields.iter().map(|(k, v)| (*k, v)));
    url.query().unwrap_or("").to_owned()
}
async fn call(
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
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn html(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
fn private(response: &Response) {
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert!(response.headers()["x-robots-tag"]
        .to_str()
        .unwrap()
        .contains("noindex"));
}
fn hidden(html: &str, name: &str) -> String {
    html.split(&format!("name=\"{name}\" value=\""))
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}
fn state_form(csrf: &str, campaign: i64, revision: i64, action: &str) -> String {
    form(&[
        ("csrf_token", csrf.into()),
        ("command_key", generate_session_token()),
        ("action", action.into()),
        ("campaign_id", campaign.to_string()),
        ("expected_revision", revision.to_string()),
    ])
}
struct LockedEditorial {
    child: Child,
    input: ChildStdin,
}
impl LockedEditorial {
    fn acquire(url: &str) -> Self {
        let mut child=Command::new("psql").env("PGOPTIONS","-c statement_timeout=5000 -c lock_timeout=2000 -c idle_in_transaction_session_timeout=10000")
            .args(["-XAtq","-v","ON_ERROR_STOP=1","--dbname",url]).stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
        let mut input = child.stdin.take().unwrap();
        writeln!(input,"BEGIN; LOCK TABLE editorial_campaigns IN ACCESS EXCLUSIVE MODE; SELECT 'editorial-locked';").unwrap();
        input.flush().unwrap();
        let mut line = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut line)
            .unwrap();
        assert_eq!(line.trim(), "editorial-locked");
        Self { child, input }
    }
}
impl Drop for LockedEditorial {
    fn drop(&mut self) {
        let _ = writeln!(self.input, "ROLLBACK;\n\\q");
        let _ = self.input.flush();
        let _ = self.child.wait();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn native_management_authority_three_real_placements_and_optional_failure() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("EditorialHttpPassword2026").unwrap();
    let mut users = Vec::new();
    let mut tokens = Vec::new();
    for role in ["administrator", "reader", "moderator"] {
        let token = generate_session_token();
        let user = accounts
            .register_with_session(
                &format!("editorial-{role}-{stamp}@example.invalid"),
                "Editorial fixture",
                &password,
                &session_token_digest(&token),
                NonZeroU32::new(3600).unwrap(),
            )
            .await
            .unwrap();
        if role != "reader" {
            pg(
                &url,
                &format!("INSERT INTO account_roles(user_id,role) VALUES({user},'{role}')"),
            );
        }
        users.push(user);
        tokens.push(token);
    }
    let csrf = generate_session_token();
    let cookie = format!("tor-dev-session={}; tor-dev-csrf={csrf}", tokens[0]);
    std::env::set_var("TOR_EDITORIAL_PROMOTION_ENABLED", "1");
    let app = tor_web::router_with_postgres_reading_auth(
        catalog.clone(),
        PgReader::new(pool.clone()),
        accounts.clone(),
        false,
        ORIGIN.into(),
    );
    for (cookie, status) in [
        (String::new(), StatusCode::UNAUTHORIZED),
        (
            format!("tor-dev-session={}", tokens[1]),
            StatusCode::FORBIDDEN,
        ),
        (
            format!("tor-dev-session={}", tokens[2]),
            StatusCode::FORBIDDEN,
        ),
    ] {
        for method in ["GET", "POST"] {
            let response = call(
                &app,
                method,
                &format!("{PAGE}?after=bad"),
                &cookie,
                ORIGIN,
                "broken",
            )
            .await;
            assert_eq!(response.status(), status);
            private(&response);
            assert!(!html(response).await.contains("data-editorial"));
        }
    }
    let promoted:i64=pg(&url,&format!("INSERT INTO books(source_identity,target_language,title,visibility,access_level) VALUES('ci:editorial-http:{stamp}:p','ru','Editorial <script>safe</script>','public','basic') RETURNING id")).parse().unwrap();
    let host:i64=pg(&url,&format!("INSERT INTO books(source_identity,target_language,title,visibility,access_level) VALUES('ci:editorial-http:{stamp}:h','ru','Editorial host','public','basic') RETURNING id")).parse().unwrap();
    let chapter:i64=pg(&url,&format!("INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount) VALUES({host},1,'Free editorial host','public','free',0) RETURNING id")).parse().unwrap();
    let version:i64=pg(&url,&format!("INSERT INTO chapter_versions(chapter_id,version_no,body_markdown) VALUES({chapter},1,'FREE HOST TEXT') RETURNING id")).parse().unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
        ),
    );
    let dates=pg(&url,"SELECT to_char((now()-interval '1 minute') AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')||'|'||to_char((now()+interval '1 hour') AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS\"Z\"')");
    let (start, end) = dates.split_once('|').unwrap();
    let before=pg(&url,"SELECT (SELECT count(*) FROM reader_wallet_entries)||'|'||(SELECT count(*) FROM reader_chapter_completions)||'|'||(SELECT count(*) FROM rec_subjects)");
    let mut campaigns = Vec::new();
    for slot in ["home_featured", "book_detail_related", "chapter_end_free"] {
        let response = call(&app, "GET", PAGE, &cookie, ORIGIN, "").await;
        assert_eq!(response.status(), StatusCode::OK);
        private(&response);
        let page = html(response).await;
        assert!(!page.contains("data-flow-proof="));
        let body = form(&[
            ("csrf_token", csrf.clone()),
            ("command_key", hidden(&page, "command_key")),
            ("action", "create".into()),
            ("expected_revision", "0".into()),
            ("book_id", promoted.to_string()),
            ("slot", slot.into()),
            ("starts_at", start.into()),
            ("ends_at", end.into()),
            ("priority", "10".into()),
        ]);
        for bad in [
            call(
                &app,
                "POST",
                PAGE,
                &cookie,
                "https://outside.invalid",
                &body,
            )
            .await,
            call(
                &app,
                "POST",
                PAGE,
                &cookie,
                ORIGIN,
                &body.replace(&csrf, "bad"),
            )
            .await,
        ] {
            assert_eq!(bad.status(), StatusCode::FORBIDDEN);
            private(&bad);
        }
        let response = call(&app, "POST", PAGE, &cookie, ORIGIN, &body).await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        private(&response);
        let location = response.headers()[header::LOCATION]
            .to_str()
            .unwrap()
            .to_owned();
        let campaign: i64 = location.split("?changed=").nth(1).unwrap().parse().unwrap();
        campaigns.push(campaign);
        let replay = call(&app, "POST", PAGE, &cookie, ORIGIN, &body).await;
        assert_eq!(replay.status(), StatusCode::SEE_OTHER);
        assert_eq!(replay.headers()[header::LOCATION], location);
        let conflict = call(
            &app,
            "POST",
            PAGE,
            &cookie,
            ORIGIN,
            &body.replace("priority=10", "priority=11"),
        )
        .await;
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        let response = call(
            &app,
            "POST",
            PAGE,
            &cookie,
            ORIGIN,
            &state_form(&csrf, campaign, 1, "activate"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            call(
                &app,
                "POST",
                PAGE,
                &cookie,
                ORIGIN,
                &state_form(&csrf, campaign, 1, "pause")
            )
            .await
            .status(),
            StatusCode::CONFLICT
        );
    }
    for route in [
        "/".to_owned(),
        format!("/books/{host}"),
        format!("/reader/{chapter}"),
    ] {
        let response = call(&app, "GET", &route, "", ORIGIN, "").await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        let page = html(response).await;
        assert!(page.contains("editorial-promotion"), "missing {route}");
        assert!(!page.contains("<script>safe</script>"));
        if route.starts_with("/reader/") {
            assert!(
                page.find("FREE HOST TEXT").unwrap() < page.find("editorial-promotion").unwrap()
            );
        }
    }
    let private_chapter = call(
        &app,
        "GET",
        &format!("/read/{chapter}"),
        &cookie,
        ORIGIN,
        "",
    )
    .await;
    assert_eq!(private_chapter.status(), StatusCode::OK);
    let private_chapter = html(private_chapter).await;
    assert!(private_chapter.contains("editorial-promotion"));
    assert!(
        private_chapter.find("FREE HOST TEXT").unwrap()
            < private_chapter.find("editorial-promotion").unwrap()
    );
    for route in ["/catalog".to_owned(), format!("/books/{promoted}")] {
        let page = html(call(&app, "GET", &route, "", ORIGIN, "").await).await;
        assert!(
            !page.contains("editorial-promotion"),
            "unapproved or self slot: {route}"
        );
    }
    let admin_page = html(call(&app, "GET", PAGE, &cookie, ORIGIN, "").await).await;
    assert!(admin_page.contains("source=editorial&amp;campaign="));
    assert!(!admin_page.contains("data-flow-proof="));
    assert!(admin_page.contains("Предпросмотр не входит в статистику"));
    for bad in [
        "after=0",
        "after=-1",
        "after=01",
        "unknown=1",
        "after=1&after=1",
    ] {
        assert_eq!(
            call(&app, "GET", &format!("{PAGE}?{bad}"), &cookie, ORIGIN, "")
                .await
                .status(),
            StatusCode::BAD_REQUEST
        );
    }
    let stopped = call(
        &app,
        "POST",
        PAGE,
        &cookie,
        ORIGIN,
        &state_form(&csrf, campaigns[0], 2, "pause"),
    )
    .await;
    assert_eq!(stopped.status(), StatusCode::SEE_OTHER);
    assert!(!html(call(&app, "GET", "/", "", ORIGIN, "").await)
        .await
        .contains("editorial-promotion"));
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={promoted}"),
    );
    assert!(
        !html(call(&app, "GET", &format!("/books/{host}"), "", ORIGIN, "").await)
            .await
            .contains("editorial-promotion")
    );
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=false WHERE id={promoted}"),
    );
    {
        let _locked = LockedEditorial::acquire(&url);
        let started = Instant::now();
        let response = call(&app, "GET", &format!("/reader/{chapter}"), "", ORIGIN, "").await;
        assert_eq!(response.status(), StatusCode::OK);
        let page = html(response).await;
        assert!(page.contains("FREE HOST TEXT"));
        assert!(!page.contains("editorial-promotion"));
        assert!(started.elapsed() < Duration::from_secs(2));
        let response = call(&app, "GET", PAGE, &cookie, ORIGIN, "").await;
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        private(&response);
    }
    std::env::remove_var("TOR_EDITORIAL_PROMOTION_ENABLED");
    let disabled = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool),
        accounts,
        false,
        ORIGIN.into(),
    );
    assert!(
        !html(call(&disabled, "GET", &format!("/books/{host}"), "", ORIGIN, "").await)
            .await
            .contains("editorial-promotion")
    );
    pg(
        &url,
        &format!(
            "UPDATE account_roles SET is_active=false WHERE user_id={} AND role='administrator'",
            users[0]
        ),
    );
    assert_eq!(
        call(&app, "GET", PAGE, &cookie, ORIGIN, "").await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            PAGE,
            &cookie,
            ORIGIN,
            &state_form(&csrf, campaigns[1], 2, "pause")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(pg(&url,"SELECT (SELECT count(*) FROM reader_wallet_entries)||'|'||(SELECT count(*) FROM reader_chapter_completions)||'|'||(SELECT count(*) FROM rec_subjects)"),before);
    pg(&url,&format!("DELETE FROM chapter_publications WHERE chapter_id={chapter}; DELETE FROM books WHERE id IN ({promoted},{host}); DELETE FROM account_roles WHERE user_id IN ({},{},{}); DELETE FROM users WHERE id IN ({},{},{})",users[0],users[1],users[2],users[0],users[1],users[2]));
}
