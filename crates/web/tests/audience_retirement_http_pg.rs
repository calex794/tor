//! Current private reports read evidence from a genuine accepted-190 to 193 upgrade.
//! This test never inserts retired raw facts or disables the installed barrier.
#![forbid(unsafe_code)]

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
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

fn pg(url: &str, statement: &str) -> String {
    let parsed = reqwest::Url::parse(url).expect("isolated fixture database URL");
    assert_eq!(parsed.scheme(), "postgresql");
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert_eq!(parsed.port(), Some(5432));
    assert_eq!(parsed.username(), "tor_app");
    assert_eq!(parsed.path(), "/tor_ci_collection_retirement_upgrade");
    assert!(parsed.query().is_none() && parsed.fragment().is_none());
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
            statement,
        ])
        .output()
        .expect("isolated installed-upgrade database");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn get(app: &Router, path: &str, cookie: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn body(response: Response, expected: StatusCode) -> String {
    assert_eq!(response.status(), expected);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let robots = response.headers()["x-robots-tag"].to_str().unwrap();
    assert!(robots.contains("noindex") && robots.contains("nofollow"));
    String::from_utf8(
        to_bytes(response.into_body(), 256 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn part<'a>(html: &'a str, marker: &str, end: &str) -> &'a str {
    html.split(marker)
        .nth(1)
        .expect("expected rendered evidence")
        .split(end)
        .next()
        .unwrap()
}

fn metric(html: &str, key: &str) -> String {
    part(html, &format!("data-metric=\"{key}\">"), "</dd>").to_owned()
}

fn snapshot(url: &str) -> String {
    pg(
        url,
        "SELECT jsonb_build_object(
         'starts',(SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]')
           FROM reader_chapter_starts t),
         'completions',(SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]')
           FROM reader_chapter_completions t),
         'preferences',(SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]')
           FROM rec_book_preferences t),
         'commands',(SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]')
           FROM rec_user_preference_commands t),
         'markers',(SELECT COALESCE(jsonb_agg(to_jsonb(t) ORDER BY to_jsonb(t)::text),'[]')
           FROM rec_analytics_invalidations t))::text",
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires the actually upgraded and drained tor_ci_collection_retirement_upgrade"]
async fn upgraded_reports_keep_unknown_evidence_private_and_unaffected_books_measured() {
    let url = std::env::var("COLLECTION_RETIREMENT_APP_DATABASE_URL")
        .expect("installed-upgrade app fixture URL");
    assert_eq!(
        pg(
            &url,
            "SELECT EXISTS(SELECT 1 FROM _sqlx_migrations
             WHERE version=202611168903 AND success)
             AND EXISTS(SELECT 1 FROM pg_trigger
             WHERE tgrelid='rec_subjects'::regclass
             AND tgname='retired_collection_write_fence' AND tgenabled='O')
             AND NOT EXISTS(SELECT 1 FROM rec_subjects)",
        ),
        "t"
    );
    let linked: i64 = pg(
        &url,
        "SELECT id FROM books WHERE source_identity='ci:retirement:linked'",
    )
    .parse()
    .unwrap();
    let untouched: i64 = pg(
        &url,
        "SELECT id FROM books WHERE source_identity='ci:retirement:untouched'",
    )
    .parse()
    .unwrap();
    let before = snapshot(&url);
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let token = generate_session_token();
    let cookie = format!("tor-dev-session={token}");
    let password = hash_password("RetirementHttpPassword2026").unwrap();
    let user = accounts
        .register_with_session(
            &format!("retirement-http-{stamp}@example.invalid"),
            "Retirement report fixture",
            &password,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let paths = [
        "/admin/audience?days=7",
        "/admin/audience/books?days=7",
        "/admin/audience/groups?kind=genres&days=7",
        "/admin/audience/visits?days=7",
    ];
    for path in paths {
        let anonymous = body(get(&app, path, "").await, StatusCode::UNAUTHORIZED).await;
        let ordinary = body(get(&app, path, &cookie).await, StatusCode::FORBIDDEN).await;
        assert!(!anonymous.contains("data-metric="));
        assert!(!ordinary.contains("data-metric="));
    }
    pg(
        &url,
        &format!("INSERT INTO account_roles(user_id,role) VALUES({user},'administrator')"),
    );
    let sources = body(get(&app, paths[0], &cookie).await, StatusCode::OK).await;
    let current = part(
        &sources,
        "data-testid=\"audience-sources-current\"",
        "</section>",
    );
    for source in ["recommendations", "catalog", "search", "unattributed"] {
        let card = part(current, &format!("data-source=\"{source}\""), "</article>");
        assert!(card.contains("data-evidence=\"invalidated\""));
        for key in ["starts", "completions"] {
            assert_eq!(metric(card, key), "Нет данных");
        }
        for key in ["clicks", "book-opens"] {
            assert_eq!(
                metric(card, key),
                if source == "unattributed" {
                    "Не измеряется"
                } else {
                    "Нет данных"
                }
            );
        }
    }

    let book_path = |id: i64| {
        if id > 1 {
            format!("/admin/audience/books?days=7&after_id={}", id - 1)
        } else {
            paths[1].to_owned()
        }
    };
    let books = body(get(&app, &book_path(linked), &cookie).await, StatusCode::OK).await;
    let linked_row = part(&books, &format!("data-book-id=\"{linked}\""), "</article>");
    let linked_current = part(linked_row, "data-period=\"current\"", "</section>");
    assert!(linked_current.contains("data-state=\"unknown-evidence\""));
    assert!(linked_current.contains("data-evidence=\"invalidated\""));
    assert_eq!(metric(linked_current, "starts"), "Нет данных");
    let untouched_html = body(
        get(&app, &book_path(untouched), &cookie).await,
        StatusCode::OK,
    )
    .await;
    let kept = part(
        &untouched_html,
        &format!("data-book-id=\"{untouched}\""),
        "</article>",
    );
    let kept_current = part(kept, "data-period=\"current\"", "</section>");
    assert!(!kept_current.contains("data-evidence=\"invalidated\""));
    for (key, value) in [
        ("issued", "22"),
        ("impressions", "13"),
        ("clicks", "15"),
        ("book-opens", "15"),
        ("starts", "14"),
    ] {
        assert_eq!(metric(kept_current, key), value);
    }
    let groups = body(get(&app, paths[2], &cookie).await, StatusCode::OK).await;
    assert!(groups.contains("data-evidence=\"invalidated\""));
    assert_eq!(
        pg(
            &url,
            "SELECT EXISTS(SELECT 1 FROM flow_visit_daily
             WHERE cohort_day>=(now() AT TIME ZONE 'UTC')::date-6)",
        ),
        "t",
        "the upgrade fixture must retain a daily bucket after its raw pages are erased"
    );
    let visits = body(get(&app, paths[3], &cookie).await, StatusCode::OK).await;
    let visit_current = part(&visits, "data-testid=\"flow-visits-current\"", "</section>");
    assert_eq!(metric(visit_current, "unique-subjects"), "Нет измерения");
    assert_eq!(
        metric(visit_current, "unattributed-pageviews"),
        "Нет измерения"
    );
    for html in [&sources, &books, &untouched_html, &groups, &visits] {
        assert!(html.contains("data-current-measurement=\"unavailable\""));
        assert!(!html.contains("retirement-main@ci.invalid"));
        assert!(!html.contains("session_digest"));
        assert!(!html.contains("subject_id"));
    }
    assert_eq!(
        snapshot(&url),
        before,
        "GET must preserve C, P1 and evidence"
    );
    pg(
        &url,
        &format!(
            "UPDATE account_roles SET is_active=false
             WHERE user_id={user} AND role='administrator'"
        ),
    );
    for path in paths {
        let denied = body(get(&app, path, &cookie).await, StatusCode::FORBIDDEN).await;
        assert!(!denied.contains("data-metric="));
    }
    pg(
        &url,
        &format!(
            "DELETE FROM account_roles WHERE user_id={user}; DELETE FROM users WHERE id={user}"
        ),
    );
    pool.close().await;
}
