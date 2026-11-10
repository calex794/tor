//! Retired FLOW-02 collection: old flags/scopes cannot re-enable HTTP tracking.
//! Historical visit facts are explicit fixtures; actual HTML remains usable.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::{
    collections::BTreeMap,
    process::Command,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tor_accounts::{repositories::PgAccounts, services::security::generate_session_token};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tor_recommendations::{
    repositories::{visits::PgFlowVisits, PgRecommendations},
    services::visits::{FlowVisitsReport, VisitObservation},
};
use tower::ServiceExt;

#[path = "support/collection_retirement.rs"]
mod retirement;
use retirement::{legacy_scope, optional_snapshot};

#[path = "flow_visits_http_pg/outage.rs"]
mod outage;
#[path = "flow_visits_http_pg/paid.rs"]
mod paid;
#[path = "flow_visits_http_pg/startup.rs"]
mod startup;

const ORIGIN: &str = "http://127.0.0.1:3098";
const ORDINARY_UA: &str = "Mozilla/5.0 Chrome/132.0.0.0 Safari/537.36";
const FLOW_FORM: &str = "/me/recommendations/flow-consent";
const REC_FORM: &str = "/me/recommendations/consent";

fn pg(url: &str, statement: &str) -> String {
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
            statement,
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

async fn text(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 2 * 1024 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[derive(Default)]
struct Browser {
    cookies: BTreeMap<String, String>,
}

impl Browser {
    fn accept(&mut self, response: &Response) {
        for cookie in response.headers().get_all(header::SET_COOKIE) {
            let cookie = cookie.to_str().unwrap().split(';').next().unwrap();
            let (name, value) = cookie.split_once('=').unwrap();
            self.cookies.insert(name.to_owned(), value.to_owned());
        }
    }

    fn cookie_header(&self) -> String {
        self.cookies
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("; ")
    }

    async fn request(
        &self,
        app: &Router,
        method: &str,
        path: &str,
        body: &str,
        extra: &[(&str, &str)],
    ) -> Response {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .extension(axum::extract::ConnectInfo(std::net::SocketAddr::from((
                [127, 0, 0, 1],
                43198,
            ))))
            .header(header::COOKIE, self.cookie_header())
            .header(header::HOST, "127.0.0.1:3098")
            .header(header::USER_AGENT, ORDINARY_UA)
            .header(header::ORIGIN, ORIGIN)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
        for (name, value) in extra {
            request.headers_mut().unwrap().insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                axum::http::HeaderValue::from_str(value).unwrap(),
            );
        }
        app.clone()
            .oneshot(request.body(Body::from(body.to_owned())).unwrap())
            .await
            .unwrap()
    }

    async fn get(&self, app: &Router, path: &str) -> Response {
        self.request(app, "GET", path, "", &[]).await
    }
}

async fn registration(app: &Router, url: &str, email: &str) -> (Browser, i64) {
    let mut browser = Browser::default();
    let page = browser.get(app, "/register").await;
    assert_eq!(page.status(), StatusCode::OK);
    browser.accept(&page);
    let body = format!(
        "email={email}&display_name=FLOW+test&password=FlowVisitsPassword2026&csrf_token={}",
        browser.cookies["tor-dev-csrf"]
    );
    let response = browser.request(app, "POST", "/register", &body, &[]).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    browser.accept(&response);
    let id = pg(
        url,
        &format!("SELECT id FROM users WHERE email_normalized='{email}'"),
    )
    .parse()
    .unwrap();
    (browser, id)
}

async fn login(app: &Router, email: &str) -> Browser {
    let mut browser = Browser::default();
    let page = browser.get(app, "/login").await;
    assert_eq!(page.status(), StatusCode::OK);
    browser.accept(&page);
    let body = format!(
        "email={email}&password=FlowVisitsPassword2026&csrf_token={}",
        browser.cookies["tor-dev-csrf"]
    );
    let response = browser.request(app, "POST", "/login", &body, &[]).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    browser.accept(&response);
    browser
}

async fn settings(browser: &Browser, app: &Router) -> String {
    let response = browser.get(app, "/me/recommendations").await;
    assert_eq!(response.status(), StatusCode::OK);
    text(response).await
}

async fn retired_choice(browser: &Browser, app: &Router, action: &str, enabled: bool) {
    let body = format!(
        "enabled={}&expected_revision=7&command_key={}&csrf_token={}",
        if enabled { "1" } else { "" },
        generate_session_token(),
        browser.cookies["tor-dev-csrf"]
    );
    let response = browser.request(app, "POST", action, &body, &[]).await;
    assert_eq!(response.status(), StatusCode::GONE);
    private(&response);
    let body: serde_json::Value = serde_json::from_str(&text(response).await).unwrap();
    assert_eq!(body["error"]["code"], "COLLECTION_RETIRED");
}

fn private(response: &Response) {
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let robots = response.headers()["x-robots-tag"].to_str().unwrap();
    assert!(robots.contains("noindex") && robots.contains("nofollow"));
}

fn own_pages(url: &str, actor: i64) -> i64 {
    pg(
        url,
        &format!(
            "SELECT COALESCE(sum(v.pageviews),0) FROM flow_visits v
        JOIN rec_subjects s ON s.subject_id=v.subject_id WHERE s.user_id={actor}"
        ),
    )
    .parse()
    .unwrap()
}

fn assert_report(html: &str, report: &FlowVisitsReport) {
    for (key, period) in [("current", &report.current), ("previous", &report.previous)] {
        let marker = format!("data-testid=\"flow-visits-{key}\"");
        let section = html
            .split(marker.as_str())
            .nth(1)
            .expect("rendered visit period");
        assert!(section.contains(&period.from_utc) && section.contains(&period.until_utc));
        for (name, expected) in [
            ("visits", period.visits),
            ("pageviews", period.pageviews),
            ("unique-subjects", period.unique_subjects),
            ("observed-seconds", period.observed_seconds),
        ] {
            let marker = format!("data-metric=\"{name}\"");
            let value = section
                .split(marker.as_str())
                .nth(1)
                .unwrap()
                .split('>')
                .nth(1)
                .unwrap()
                .split('<')
                .next()
                .unwrap();
            assert_eq!(
                value,
                expected
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| "Нет измерения".to_owned())
            );
        }
        for row in &period.rows {
            let marker = format!(
                "data-source=\"{}\" data-landing=\"{}\" data-last=\"{}\"",
                row.acquisition_hint, row.landing_surface, row.last_surface
            );
            assert!(section.contains(&marker), "missing source/landing/last row");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn retired_flow_keeps_navigation_historical_reports_and_live_admin_guards() {
    for name in [
        "TOR_FLOW_VISITS_ENABLED",
        "TOR_RECOMMENDATIONS_ENABLED",
        "TOR_REC_ATTRIBUTION_ENABLED",
        "TOR_ORGANIC_ATTRIBUTION_ENABLED",
        "TOR_READING_RETURNS_ENABLED",
        "TOR_BOOK_READING_RETURNS_ENABLED",
        "TOR_RIL_SINGLE_INSTANCE",
        "TOR_FLOW_PUBLIC_OPT_IN_ENABLED",
    ] {
        std::env::set_var(name, "1");
    }
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let store = PgRecommendations::new(pool.clone());
    let visits = PgFlowVisits::new(pool.clone());
    let app = tor_web::router_with_postgres_reading_auth(
        catalog.clone(),
        PgReader::new(pool.clone()),
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let admin_email = format!("flow-admin-{stamp}@tor-ci.example");
    let (_, admin) = registration(&app, &url, &admin_email).await;
    let admin_browser = login(&app, &admin_email).await;
    let (reader_browser, reader) =
        registration(&app, &url, &format!("flow-reader-{stamp}@tor-ci.example")).await;
    let (moderator_browser, moderator) = registration(
        &app,
        &url,
        &format!("flow-moderator-{stamp}@tor-ci.example"),
    )
    .await;
    pg(&url, &format!("INSERT INTO account_roles(user_id,role) VALUES({admin},'administrator'),({moderator},'moderator')"));
    let book: i64 = pg(&url, &format!("INSERT INTO books(source_identity,target_language,title,visibility,access_level)
        VALUES('ci:flow:visits:http:{stamp}','ru','FLOW-PRIVATE-{stamp}','public','basic') RETURNING id")).parse().unwrap();
    let chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
        VALUES({book},1,'FLOW test chapter','public','free',0) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
        VALUES({chapter},1,'FLOW readable fixture') RETURNING id"
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
    let guest = Browser::default();
    for (browser, status) in [
        (&guest, StatusCode::UNAUTHORIZED),
        (&reader_browser, StatusCode::FORBIDDEN),
        (&moderator_browser, StatusCode::FORBIDDEN),
    ] {
        let response = browser
            .get(&app, "/admin/audience/visits?days=invalid")
            .await;
        assert_eq!(response.status(), status);
        private(&response);
        assert!(!text(response).await.contains("data-metric="));
    }
    let html = settings(&reader_browser, &app).await;
    assert!(!html.contains("data-flow-consent"));
    assert!(!html.contains(REC_FORM));
    assert!(!html.contains(FLOW_FORM));
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM rec_subjects WHERE user_id={reader}")
        ),
        "0"
    );
    for endpoint in [REC_FORM, FLOW_FORM] {
        let body = format!(
            "enabled=1&expected_revision=0&command_key={}&csrf_token={}",
            generate_session_token(),
            reader_browser.cookies["tor-dev-csrf"]
        );
        for (payload, extra) in [
            (body.clone(), vec![("origin", "https://attacker.invalid")]),
            (
                body.replace(&reader_browser.cookies["tor-dev-csrf"], "invalid"),
                vec![],
            ),
        ] {
            let response = reader_browser
                .request(&app, "POST", endpoint, &payload, &extra)
                .await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN);
            private(&response);
        }
        let mut guest_with_csrf = Browser::default();
        guest_with_csrf.cookies.insert(
            "tor-dev-csrf".into(),
            reader_browser.cookies["tor-dev-csrf"].clone(),
        );
        let response = guest_with_csrf
            .request(&app, "POST", endpoint, &body, &[])
            .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        private(&response);
        retired_choice(&reader_browser, &app, endpoint, true).await;
    }
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM rec_subjects WHERE user_id={reader}")
        ),
        "0",
        "retired endpoints cannot create a previously absent subject"
    );

    // Explicit legacy installation fixture, separate from every HTTP request.
    legacy_scope(&url, reader);
    pg(
        &url,
        &format!("INSERT INTO rec_subjects(user_id,enabled,revision) VALUES({moderator},false,9)"),
    );
    let disabled_scope = optional_snapshot(&url, moderator);
    let scope = store.flow_scope(reader).await.unwrap().unwrap();
    let historical = VisitObservation::new(
        scope.clone(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64,
        1,
        0,
    )
    .unwrap();
    assert!(visits.observe(&historical).await.unwrap());
    let baseline = optional_snapshot(&url, reader);
    assert_eq!(own_pages(&url, reader), 1);

    for path in [
        "/".to_owned(),
        "/catalog?sort=newest".to_owned(),
        format!("/books/{book}"),
        format!("/reader/{chapter}"),
        format!("/read/{chapter}"),
        "/me/library".to_owned(),
    ] {
        let response = reader_browser
            .request(
                &app,
                "GET",
                &path,
                "",
                &[(
                    "referer",
                    "https://www.yandex.ru/search/?text=FLOW-SECRET-QUERY",
                )],
            )
            .await;
        assert_eq!(response.status(), StatusCode::OK, "{path}");
        let html = text(response).await;
        for hook in [
            "data-rec-ticket",
            "data-organic-ticket",
            "/assets/recommendations.js",
            "/assets/organic_discovery.js",
        ] {
            assert!(!html.contains(hook), "retired hook in {path}: {hook}");
        }
        assert_eq!(optional_snapshot(&url, reader), baseline, "{path}");
    }
    // This constructor does not mount the private paid reader. The real
    // checkout + purchased-library 200 path is exercised by paid.rs.
    assert_eq!(
        reader_browser.get(&app, "/me/purchases").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(optional_snapshot(&url, reader), baseline);
    for _ in 0..2 {
        let restarted = tor_web::router_with_postgres_reading_auth(
            catalog.clone(),
            PgReader::new(pool.clone()),
            accounts.clone(),
            false,
            ORIGIN.to_owned(),
        );
        assert_eq!(
            reader_browser
                .get(&restarted, &format!("/books/{book}"))
                .await
                .status(),
            StatusCode::OK
        );
        assert_eq!(optional_snapshot(&url, reader), baseline);
    }
    for browser in [&guest, &moderator_browser] {
        assert_eq!(
            browser.get(&app, &format!("/books/{book}")).await.status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        optional_snapshot(&url, moderator),
        disabled_scope,
        "an existing off scope is not automatically enabled"
    );
    assert_eq!(
        reader_browser
            .request(
                &app,
                "GET",
                &format!("/books/{book}"),
                "",
                &[("user-agent", "Googlebot/2.1")]
            )
            .await
            .status(),
        StatusCode::OK
    );
    for path in ["/api/v1/books?limit=1", "/assets/site.css"] {
        assert_eq!(
            reader_browser.get(&app, path).await.status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        reader_browser
            .get(&app, "/books/9223372036854775807")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        reader_browser
            .request(&app, "HEAD", "/catalog", "", &[])
            .await
            .status(),
        StatusCode::OK
    );
    for endpoint in [REC_FORM, FLOW_FORM] {
        retired_choice(&reader_browser, &app, endpoint, false).await;
        retired_choice(&reader_browser, &app, endpoint, true).await;
    }
    assert_eq!(
        optional_snapshot(&url, reader),
        baseline,
        "legacy consent rows, timestamps and historical measurements stay unchanged in P2a"
    );
    for days in [1, 7, 30, 90] {
        let report = visits.report(days).await.unwrap();
        let response = admin_browser
            .get(&app, &format!("/admin/audience/visits?days={days}"))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        private(&response);
        let html = text(response).await;
        assert!(html.contains("data-testid=\"collection-retired\""));
        assert!(html.contains("data-current-measurement=\"unavailable\""));
        assert!(html.contains("data-measurement=\"legacy-consented\""));
        assert_report(&html, &report);
        for forbidden in [
            "FLOW-PRIVATE-",
            "FLOW-SECRET-QUERY",
            "FLOW readable fixture",
            &admin_email,
            &scope.subject_id,
            &scope.epoch,
        ] {
            assert!(
                !html.contains(forbidden),
                "identity or raw content leaked into report"
            );
        }
        if days == 90 {
            assert_eq!(report.current.unique_subjects, None);
        }
    }
    assert_eq!(
        admin_browser
            .get(&app, "/admin/audience/visits?days=2")
            .await
            .status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(optional_snapshot(&url, reader), baseline);
    pg(&url, &format!("UPDATE account_roles SET is_active=false WHERE user_id={admin} AND role='administrator'"));
    let denied = admin_browser.get(&app, "/admin/audience/visits").await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    private(&denied);
    assert!(!text(denied).await.contains("data-metric="));
    pg(
        &url,
        &format!(
            "BEGIN; DELETE FROM account_roles WHERE user_id IN({admin},{moderator});
        DELETE FROM chapter_publications WHERE chapter_id={chapter};
        DELETE FROM book_members WHERE book_id={book}; DELETE FROM books WHERE id={book};
        DELETE FROM users WHERE id IN({admin},{reader},{moderator}); COMMIT"
        ),
    );
}
