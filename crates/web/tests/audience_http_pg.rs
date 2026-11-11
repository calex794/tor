//! FLOW: a live administrator reads C facts; a pageview never becomes reading.
//! Runs only against disposable PostgreSQL. It does not enable REC profiling.
#![forbid(unsafe_code)]
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

fn pg(url: &str, sql: &str) -> String {
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let output = Command::new("psql")
        .env(
            "PGOPTIONS",
            "-c statement_timeout=5000 -c lock_timeout=2000",
        )
        .args([
            "-X",
            "-A",
            "-t",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            url,
            "--command",
            sql,
        ])
        .output()
        .expect("disposable PostgreSQL client");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    csrf: &str,
    payload: &str,
) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-csrf-token", csrf)
                .body(Body::from(payload.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn page(response: Response) -> String {
    String::from_utf8(
        // The bounded 168-hour comparison adds its accessible table to this page.
        to_bytes(response.into_body(), 256 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn private(response: &Response) {
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let robots = response.headers()["x-robots-tag"].to_str().unwrap();
    assert!(robots.contains("noindex") && robots.contains("nofollow"));
}

fn metric(html: &str, period: &str, metric: &str) -> i64 {
    let section_marker = format!("data-testid=\"audience-{period}\"");
    let section = html
        .split(section_marker.as_str())
        .nth(1)
        .expect("period section")
        .split("</section>")
        .next()
        .unwrap();
    let metric_marker = format!("data-metric=\"{metric}\"");
    let Some(value) = section.split(metric_marker.as_str()).nth(1) else {
        // An empty source has no numeric metrics. This fallback is only a
        // fixture delta baseline; the product must not render a fake zero.
        assert!(section.contains("нет") || section.contains("Нет"));
        return 0;
    };
    value
        .split('>')
        .nth(1)
        .unwrap()
        .split('<')
        .next()
        .unwrap()
        .trim()
        .parse()
        .expect("numeric aggregate")
}

async fn progress(
    app: &Router,
    cookie: &str,
    csrf: &str,
    chapter: i64,
    version: i64,
    position: (i32, i64),
) -> Result<i64, StatusCode> {
    let response = call(
        app,
        "PUT",
        "/api/v1/me/progress/free",
        cookie,
        csrf,
        &json!({
            "chapter_id": chapter.to_string(), "version_id": version.to_string(),
            "block_id": 1, "char_offset": position.0, "expected_revision": position.1,
        })
        .to_string(),
    )
    .await;
    if response.status() != StatusCode::OK {
        return Err(response.status());
    }
    Ok(
        serde_json::from_str::<Value>(&page(response).await).unwrap()["revision"]
            .as_i64()
            .unwrap(),
    )
}

// A rejected C progress transaction rolls back asynchronously. Keep its driver
// running while synchronous psql waits for the transaction's shared Book lock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn audience_admin_live_authority_real_c_progress_replay_and_database_failure() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("AudienceHttpTestPassword2026").unwrap();
    let mut users = Vec::new();
    let mut cookies = Vec::new();
    let mut tokens = Vec::new();
    for kind in ["administrator", "reader", "moderator"] {
        let token = generate_session_token();
        let user = accounts
            .register_with_session(
                &format!("audience-{kind}-{stamp}@example.invalid"),
                "Audience fixture",
                &password,
                &session_token_digest(&token),
                NonZeroU32::new(3600).unwrap(),
            )
            .await
            .unwrap();
        users.push(user);
        cookies.push(format!("tor-dev-session={token}"));
        tokens.push(token);
    }
    let admin = users[0];
    let reader = users[1];
    let moderator = users[2];
    pg(&url, &format!(
        "INSERT INTO account_roles(user_id,role) VALUES({admin},'administrator'),({moderator},'moderator')"
    ));
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );
    for (cookie, status) in [
        ("", StatusCode::UNAUTHORIZED),
        (cookies[1].as_str(), StatusCode::FORBIDDEN),
        (cookies[2].as_str(), StatusCode::FORBIDDEN),
    ] {
        let denied = call(&app, "GET", "/admin/audience", cookie, "", "").await;
        assert_eq!(denied.status(), status);
        private(&denied);
        assert!(!page(denied).await.contains("data-metric="));
    }
    let denied = call(&app, "GET", "/admin/audience?days=bad", "", "", "").await;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    private(&denied);
    let before = call(&app, "GET", "/admin/audience?days=7", &cookies[0], "", "").await;
    assert_eq!(before.status(), StatusCode::OK);
    private(&before);
    let before = page(before).await;

    // Reading needs no author ownership. An unowned catalogue fixture avoids
    // creating immutable revenue-share history unrelated to this C scenario.
    let book: i64 = pg(&url, &format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES('ci:audience:http:{stamp}','ru','AUDIENCE-HIDDEN-TITLE-{stamp}','public','basic') RETURNING id"
    )).parse().unwrap();
    let chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES({book},1,'Audience fixture chapter','public','free',0) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES({chapter},1,'Audience text') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
        ),
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/reader/{chapter}"),
            &cookies[1],
            "",
            ""
        )
        .await
        .status(),
        StatusCode::OK
    );
    let opened = page(call(&app, "GET", "/admin/audience?days=7", &cookies[0], "", "").await).await;
    assert_eq!(
        metric(&opened, "current", "starts"),
        metric(&before, "current", "starts")
    );
    assert_eq!(
        metric(&opened, "current", "completions"),
        metric(&before, "current", "completions")
    );

    let csrf = generate_session_token();
    let reader_cookie = format!("{}; tor-dev-csrf={csrf}", cookies[1]);
    let revision = progress(&app, &reader_cookie, &csrf, chapter, version, (3, 0))
        .await
        .unwrap();
    let revision = progress(
        &app,
        &reader_cookie,
        &csrf,
        chapter,
        version,
        (13, revision),
    )
    .await
    .unwrap();
    assert_eq!(
        progress(
            &app,
            &reader_cookie,
            &csrf,
            chapter,
            version,
            (13, revision)
        )
        .await,
        Err(StatusCode::CONFLICT),
        "repeating the same position must not create another first reading fact"
    );
    assert_eq!(pg(&url, &format!("SELECT count(*) FROM reader_chapter_starts WHERE user_id={reader} AND chapter_id={chapter}")), "1");
    assert_eq!(pg(&url, &format!("SELECT count(*) FROM reader_chapter_completions WHERE user_id={reader} AND chapter_id={chapter}")), "1");
    let source = pg(&url, &format!("SELECT source_event_id FROM reader_chapter_completions WHERE user_id={reader} AND chapter_id={chapter}"));
    intent::choices(&app, (&cookies[0], &reader_cookie, &csrf), book).await;
    canonical::check(&app, &url, &cookies, book, stamp, &source).await;
    // A real administrative hold hides content without rewriting past C facts.
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={book}"),
    );
    let after = call(&app, "GET", "/admin/audience?days=7", &cookies[0], "", "").await;
    assert_eq!(after.status(), StatusCode::OK);
    private(&after);
    let after = page(after).await;
    for metric_name in ["starts", "completions", "readers"] {
        assert_eq!(
            metric(&after, "current", metric_name),
            metric(&before, "current", metric_name) + 1
        );
        assert_eq!(
            metric(&after, "previous", metric_name),
            metric(&before, "previous", metric_name)
        );
    }
    assert!(!after.contains("AUDIENCE-HIDDEN-TITLE"));
    assert!(!after.contains(&source));
    assert!(!after.contains(&format!("audience-reader-{stamp}@example.invalid")));
    assert!(!after.contains("Audience text"));
    for days in [1, 30, 90] {
        let response = call(
            &app,
            "GET",
            &format!("/admin/audience?days={days}"),
            &cookies[0],
            "",
            "",
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        private(&response);
        assert!(page(response).await.contains("audience-current"));
    }
    for query in ["days=0", "days=365", "days=bad", "unexpected=1"] {
        let response = call(
            &app,
            "GET",
            &format!("/admin/audience?{query}"),
            &cookies[0],
            "",
            "",
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        private(&response);
    }
    pg(&url, &format!("UPDATE account_roles SET is_active=false WHERE user_id={admin} AND role='administrator'"));
    let revoked = call(&app, "GET", "/admin/audience", &cookies[0], "", "").await;
    assert_eq!(revoked.status(), StatusCode::FORBIDDEN);
    private(&revoked);
    let revoked = call(&app, "GET", "/admin/audience/reading", &cookies[0], "", "").await;
    assert_eq!(revoked.status(), StatusCode::FORBIDDEN);
    private(&revoked);
    pg(&url, &format!("UPDATE account_roles SET is_active=true WHERE user_id={admin} AND role='administrator'"));
    accounts
        .revoke_session(&session_token_digest(&tokens[0]))
        .await
        .unwrap();
    let revoked = call(&app, "GET", "/admin/audience", &cookies[0], "", "").await;
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
    private(&revoked);

    let revoked = call(&app, "GET", "/admin/audience/reading", &cookies[0], "", "").await;
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
    private(&revoked);

    // A failed database is unavailable, never an empty successful report.
    pool.close().await;
    let failed = call(&app, "GET", "/admin/audience", &cookies[0], "", "").await;
    assert_eq!(failed.status(), StatusCode::SERVICE_UNAVAILABLE);
    private(&failed);
    assert!(!page(failed).await.contains("data-metric="));
    pg(
        &url,
        &format!(
            "BEGIN;
         DELETE FROM account_roles WHERE user_id IN({admin},{reader},{moderator});
         DELETE FROM reader_book_positions WHERE book_id={book};
         DELETE FROM reader_chapter_starts WHERE book_id={book};
         DELETE FROM reader_chapter_completions WHERE book_id={book};
         DELETE FROM reader_saved_free_chapters WHERE user_id={reader} AND chapter_id IN(SELECT id FROM chapters WHERE book_id={book});
         DELETE FROM chapter_publications WHERE chapter_id IN(SELECT id FROM chapters WHERE book_id={book});
         DELETE FROM book_members WHERE book_id={book};
         DELETE FROM books WHERE id={book};
         DELETE FROM users WHERE id IN({admin},{reader},{moderator});
         COMMIT"
        ),
    );
}

#[path = "audience_http_pg/intent.rs"]
mod intent;

#[path = "audience_http_pg/canonical.rs"]
mod canonical;
