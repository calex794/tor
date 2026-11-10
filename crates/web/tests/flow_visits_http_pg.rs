//! FLOW-02: real account forms and successful HTML navigation feed the queue.
//! Only books/accounts are fixtures; positive visit facts come from HTTP.
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
use tor_accounts::repositories::PgAccounts;
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tor_recommendations::{
    repositories::{visits::PgFlowVisits, PgRecommendations},
    services::visits::{FlowVisitsReport, VisitObservation},
};
use tower::ServiceExt;

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

fn field(form: &str, name: &str) -> String {
    let name = format!("name=\"{name}\"");
    let input = form
        .split('<')
        .find(|tag| tag.starts_with("input ") && tag.contains(&name))
        .unwrap();
    input
        .split("value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}

fn form(html: &str, action: &str) -> String {
    let action = format!("action=\"{action}\"");
    html.split(action.as_str())
        .nth(1)
        .unwrap()
        .split("</form>")
        .next()
        .unwrap()
        .to_owned()
}

fn command(form: &str, enabled: bool) -> String {
    format!(
        "enabled={}&expected_revision={}&command_key={}&csrf_token={}",
        if enabled { "1" } else { "0" },
        field(form, "expected_revision"),
        field(form, "command_key"),
        field(form, "csrf_token")
    )
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

async fn set_choice(browser: &Browser, app: &Router, action: &str, enabled: bool) {
    let html = settings(browser, app).await;
    let body = command(&form(&html, action), enabled);
    let response = browser.request(app, "POST", action, &body, &[]).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
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

async fn wait_pages(url: &str, actor: i64, expected: i64) {
    for _ in 0..100 {
        let pages = own_pages(url, actor);
        assert!(
            pages <= expected,
            "duplicate/extra visit capture: {pages} > {expected}"
        );
        if pages == expected {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("FLOW queue did not save {expected} authorized HTML pages");
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
async fn flow_visits_http_consent_navigation_restart_erasure_and_live_admin() {
    for name in [
        "TOR_FLOW_VISITS_ENABLED",
        "TOR_RECOMMENDATIONS_ENABLED",
        "TOR_REC_ATTRIBUTION_ENABLED",
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
    let initial = form(&html, FLOW_FORM);
    assert!(initial.contains("data-flow-consent"));
    assert!(!initial.contains(" checked"));
    assert_eq!(
        reader_browser
            .request(&app, "POST", FLOW_FORM, &command(&initial, true), &[])
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    set_choice(&reader_browser, &app, REC_FORM, true).await;
    assert!(!store.flow_consent(reader).await.unwrap().enabled);
    assert_eq!(
        reader_browser
            .get(&app, &format!("/books/{book}"))
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        own_pages(&url, reader),
        0,
        "REC opt-in must not imply FLOW permission"
    );

    let html = settings(&reader_browser, &app).await;
    let flow_form = form(&html, FLOW_FORM);
    let enable = command(&flow_form, true);
    for (payload, extra) in [
        (enable.clone(), vec![("origin", "https://attacker.invalid")]),
        (
            enable.replace(&field(&flow_form, "csrf_token"), "invalid"),
            vec![],
        ),
    ] {
        assert_eq!(
            reader_browser
                .request(&app, "POST", FLOW_FORM, &payload, &extra)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        assert!(!store.flow_consent(reader).await.unwrap().enabled);
    }
    for _ in 0..2 {
        assert_eq!(
            reader_browser
                .request(&app, "POST", FLOW_FORM, &enable, &[])
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
    }
    assert_eq!(
        store.flow_consent(reader).await.unwrap().revision,
        2,
        "form replay changes no consent revision"
    );
    let scope = store.flow_scope(reader).await.unwrap().unwrap();
    let first = reader_browser
        .request(
            &app,
            "GET",
            "/catalog?sort=newest",
            "",
            &[(
                "referer",
                "https://www.yandex.ru/search/?text=FLOW-SECRET-QUERY",
            )],
        )
        .await;
    assert_eq!(first.status(), StatusCode::OK);
    wait_pages(&url, reader, 1).await;
    for (index, path) in [
        format!("/books/{book}"),
        format!("/reader/{chapter}"),
        format!("/read/{chapter}"),
        "/me/library".to_owned(),
        "/".to_owned(),
    ]
    .iter()
    .enumerate()
    {
        assert_eq!(
            reader_browser
                .request(
                    &app,
                    "GET",
                    path,
                    "",
                    &[("referer", "http://127.0.0.1:3098/catalog")]
                )
                .await
                .status(),
            StatusCode::OK,
            "consented page {path}"
        );
        wait_pages(&url, reader, index as i64 + 2).await;
    }
    let app_after_restart = tor_web::router_with_postgres_reading_auth(
        catalog.clone(),
        PgReader::new(pool.clone()),
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );
    assert_eq!(
        reader_browser
            .get(&app_after_restart, &format!("/books/{book}"))
            .await
            .status(),
        StatusCode::OK
    );
    wait_pages(&url, reader, 7).await;
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*),min(acquisition_hint),min(landing_surface),min(last_surface)
        FROM flow_visits WHERE subject_id='{}'::uuid",
                scope.subject_id
            )
        ),
        "1|0|1|2"
    );

    // Every excluded request is followed by a real enqueued page as a barrier.
    let total_before = pg(&url, "SELECT count(*) FROM flow_visit_receipts")
        .parse::<i64>()
        .unwrap();
    assert_eq!(
        guest.get(&app, &format!("/books/{book}")).await.status(),
        StatusCode::OK
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
    for flag in [
        "TOR_FLOW_VISITS_ENABLED",
        "TOR_RECOMMENDATIONS_ENABLED",
        "TOR_REC_ATTRIBUTION_ENABLED",
    ] {
        std::env::remove_var(flag);
        let disabled = tor_web::router_with_postgres_reading_auth(
            catalog.clone(),
            PgReader::new(pool.clone()),
            accounts.clone(),
            false,
            ORIGIN.to_owned(),
        );
        std::env::set_var(flag, "1");
        assert_eq!(
            reader_browser
                .get(&disabled, &format!("/books/{book}"))
                .await
                .status(),
            StatusCode::OK
        );
    }
    assert_eq!(
        reader_browser
            .get(&app_after_restart, &format!("/read/{chapter}"))
            .await
            .status(),
        StatusCode::OK
    );
    wait_pages(&url, reader, 8).await;
    assert_eq!(
        pg(&url, "SELECT count(*) FROM flow_visit_receipts")
            .parse::<i64>()
            .unwrap(),
        total_before + 1
    );

    for days in [1, 7, 30, 90] {
        let report = visits.report(days).await.unwrap();
        let response = admin_browser
            .get(&app, &format!("/admin/audience/visits?days={days}"))
            .await;
        assert_eq!(response.status(), StatusCode::OK);
        private(&response);
        let html = text(response).await;
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
                "identity or raw content leaked into aggregate report"
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
    let stale = VisitObservation::new(
        scope.clone(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64,
        2,
        3,
    )
    .unwrap();
    set_choice(&reader_browser, &app, FLOW_FORM, false).await;
    assert!(
        store.consent(reader).await.unwrap().enabled,
        "FLOW opt-out preserves independent REC choice"
    );
    assert_eq!(own_pages(&url, reader), 0);
    assert!(
        !visits.observe(&stale).await.unwrap(),
        "queued old scope cannot recreate erased visits"
    );
    for table in [
        "flow_visits",
        "flow_visit_receipts",
        "flow_visit_account_days",
    ] {
        assert_eq!(
            pg(
                &url,
                &format!(
                    "SELECT count(*) FROM {table} WHERE subject_id='{}'::uuid",
                    scope.subject_id
                )
            ),
            "0"
        );
    }
    set_choice(&reader_browser, &app, FLOW_FORM, true).await;
    let new_scope = store.flow_scope(reader).await.unwrap().unwrap();
    assert_ne!(new_scope.epoch, scope.epoch);
    assert_eq!(
        reader_browser
            .get(&app, &format!("/books/{book}"))
            .await
            .status(),
        StatusCode::OK
    );
    wait_pages(&url, reader, 1).await;
    set_choice(&reader_browser, &app, REC_FORM, false).await;
    assert!(store.flow_scope(reader).await.unwrap().is_none());
    assert_eq!(own_pages(&url, reader), 0);
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
