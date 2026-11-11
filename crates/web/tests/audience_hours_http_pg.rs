//! FLOW-04: live administrator access, real hourly records and a locked RIL read.
//! Historical records below are explicit report fixtures, not proof of visits.
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
use tor_traffic_behavior::{
    repositories::PgFlowAudience,
    services::audience::{FlowHourlyPeriod, FlowHourlyReport},
};
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";
const SURFACES: [&str; 4] = ["home", "catalog", "book", "reader"];

fn client(url: &str) -> Command {
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let mut command = Command::new("psql");
    command
        .env(
            "PGOPTIONS",
            "-c statement_timeout=5000 -c lock_timeout=2000 -c idle_in_transaction_session_timeout=10000",
        )
        .args(["-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1", "--dbname", url]);
    command
}

fn pg(url: &str, sql: &str) -> String {
    let output = client(url).args(["--command", sql]).output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

struct HourFixtures {
    url: String,
    predicate: String,
    saved: String,
}

impl HourFixtures {
    fn install(url: &str, anchor: i64) -> Self {
        let current = anchor - 3 * 3600;
        let previous = anchor - 171 * 3600;
        let predicate = format!("bucket IN(to_timestamp({current}),to_timestamp({previous}))");
        let saved = pg(url, &format!(
            "SELECT extract(epoch FROM bucket)::bigint,surface_code,acquisition_hint,html_requests,
             opted_in_html_requests,observed_opted_in_visit_starts FROM ril_flow_hourly WHERE {predicate}"
        ));
        let fixtures = Self {
            url: url.to_owned(),
            predicate,
            saved,
        };
        pg(
            url,
            &format!(
                "BEGIN; DELETE FROM ril_flow_hourly WHERE {};
             INSERT INTO ril_flow_hourly VALUES
             (to_timestamp({current}),0,0,2,0,0),(to_timestamp({current}),0,1,3,0,0),
             (to_timestamp({current}),1,0,0,0,0),(to_timestamp({current}),3,3,7,0,0),
             (to_timestamp({current}),4,0,999999,0,0),
             (to_timestamp({previous}),0,0,11,0,0),(to_timestamp({previous}),2,0,13,0,0),
             (to_timestamp({previous}),3,0,0,0,0); COMMIT",
                fixtures.predicate
            ),
        );
        fixtures
    }
}

impl Drop for HourFixtures {
    fn drop(&mut self) {
        let mut restore = format!(
            "BEGIN; DELETE FROM ril_flow_hourly WHERE {};",
            self.predicate
        );
        for line in self.saved.lines() {
            let row: Vec<i64> = line
                .split('|')
                .map(|value| value.parse().unwrap())
                .collect();
            assert_eq!(row.len(), 6);
            restore.push_str(&format!(
                "INSERT INTO ril_flow_hourly VALUES(to_timestamp({}),{},{},{},{},{});",
                row[0], row[1], row[2], row[3], row[4], row[5]
            ));
        }
        restore.push_str("COMMIT");
        pg(&self.url, &restore);
    }
}

struct LockedHours {
    child: Child,
    input: ChildStdin,
}

impl LockedHours {
    fn acquire(url: &str) -> Self {
        let mut child = client(url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        writeln!(
            input,
            "BEGIN; LOCK TABLE ril_flow_hourly IN ACCESS EXCLUSIVE MODE; SELECT 'hours-locked';"
        )
        .unwrap();
        input.flush().unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "hours-locked");
        Self { child, input }
    }
}

impl Drop for LockedHours {
    fn drop(&mut self) {
        let _ = writeln!(self.input, "ROLLBACK;\n\\q");
        let _ = self.input.flush();
        let _ = self.child.wait();
    }
}

async fn call(app: &Router, path: &str, cookie: &str) -> Response {
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

async fn body(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 256 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn private(response: &Response, status: StatusCode) {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow");
}

fn metric<'a>(html: &'a str, name: &str) -> &'a str {
    let marker = format!("data-metric=\"{name}\"");
    html.split(marker.as_str())
        .nth(1)
        .unwrap()
        .split('>')
        .nth(1)
        .unwrap()
        .split('<')
        .next()
        .unwrap()
        .trim()
}

fn period_summary(html: &str, key: &str, period: &FlowHourlyPeriod) {
    let marker = format!("data-testid=\"audience-hours-{key}\"");
    let section = html
        .split(marker.as_str())
        .nth(1)
        .expect("hour summary")
        .split("</section>")
        .next()
        .unwrap();
    for (index, surface) in SURFACES.iter().enumerate() {
        let marker = format!("data-surface=\"{surface}\"");
        let card = section
            .split(marker.as_str())
            .nth(1)
            .unwrap()
            .split("data-surface=")
            .next()
            .unwrap();
        assert_eq!(
            metric(card, "saved-views"),
            period.saved_views[index]
                .map(|v| v.to_string())
                .unwrap_or_else(|| "Нет данных".into())
        );
        assert_eq!(
            metric(card, "recorded-hours"),
            period.recorded_hours[index].to_string()
        );
        assert_eq!(metric(card, "opted-in-requests"), "Не измеряется");
        assert_eq!(metric(card, "observed-visit-starts"), "Не измеряется");
    }
}

fn report_matches(html: &str, report: &FlowHourlyReport) {
    period_summary(html, "current", &report.current);
    period_summary(html, "previous", &report.previous);
    assert_eq!(
        html.matches("data-hour=\"").count(),
        report.current.hours.len()
    );
    for (current, previous) in report.current.hours.iter().zip(&report.previous.hours) {
        let marker = format!("data-hour=\"{}\"", current.hour_epoch);
        let row = html
            .split(marker.as_str())
            .nth(1)
            .expect("hour row")
            .split("</tr>")
            .next()
            .unwrap();
        for (index, surface) in SURFACES.iter().enumerate() {
            for (key, hour) in [("current", current), ("previous", previous)] {
                let surface_marker = format!("data-surface=\"{surface}\"");
                let period_marker = format!("data-period=\"{key}\"");
                let cell = row
                    .split("<td")
                    .skip(1)
                    .find(|cell| cell.contains(&surface_marker) && cell.contains(&period_marker))
                    .unwrap();
                let value = cell
                    .split('>')
                    .nth(1)
                    .unwrap()
                    .split('<')
                    .next()
                    .unwrap()
                    .trim();
                assert_eq!(
                    value,
                    hour.views[index]
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "—".into())
                );
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn flow_hours_live_admin_periods_gaps_and_isolated_database_timeout() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let repo = PgFlowAudience::new(pool.clone());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("AudienceHoursTestPassword2026").unwrap();
    let mut users = Vec::new();
    let mut cookies = Vec::new();
    let mut tokens = Vec::new();
    for kind in ["administrator", "reader", "moderator"] {
        let token = generate_session_token();
        users.push(
            accounts
                .register_with_session(
                    &format!("audience-hours-{kind}-{stamp}@example.invalid"),
                    "Hours fixture",
                    &password,
                    &session_token_digest(&token),
                    NonZeroU32::new(3600).unwrap(),
                )
                .await
                .unwrap(),
        );
        cookies.push(format!("tor-dev-session={token}"));
        tokens.push(token);
    }
    let [admin, reader, moderator] = [users[0], users[1], users[2]];
    pg(&url, &format!("INSERT INTO account_roles(user_id,role) VALUES({admin},'administrator'),({moderator},'moderator')"));
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts.clone(),
        false,
        ORIGIN.into(),
    );
    for (cookie, status) in [
        ("", StatusCode::UNAUTHORIZED),
        (cookies[1].as_str(), StatusCode::FORBIDDEN),
        (cookies[2].as_str(), StatusCode::FORBIDDEN),
    ] {
        let response = call(&app, "/admin/audience?days=7", cookie).await;
        private(&response, status);
        assert!(!body(response).await.contains("audience-hours-current"));
    }
    let anchor = repo.report(7).await.unwrap().current.until_epoch;
    let fixtures = HourFixtures::install(&url, anchor);
    for days in [1, 7] {
        let report = repo.report(days).await.unwrap();
        let response = call(&app, &format!("/admin/audience?days={days}"), &cookies[0]).await;
        private(&response, StatusCode::OK);
        let html = body(response).await;
        report_matches(&html, &report);
        assert!(html.contains("data-testid=\"audience-data-quality\""));
        assert!(html.contains("Недостоверность измерений"));
        assert!(html.contains("другие процессы и потери до запуска неизвестны"));
        assert!(
            !html.contains("999999"),
            "private-page fixture must be excluded"
        );
        assert!(
            !html.contains("audience-hours-reader-"),
            "no account identity in aggregates"
        );
        let fixture = report
            .current
            .hours
            .iter()
            .find(|hour| hour.hour_epoch == anchor - 3 * 3600)
            .unwrap();
        assert_eq!(fixture.views, [Some(5), Some(0), None, Some(7)]);
    }
    for days in [30, 90] {
        let response = call(&app, &format!("/admin/audience?days={days}"), &cookies[0]).await;
        private(&response, StatusCode::OK);
        let html = body(response).await;
        assert!(html.contains("audience-hours-unsupported"));
        // Both projections share presentation classes; only the legacy table
        // is absent for long periods. The observation-hour report stays live.
        assert!(!html.contains("data-testid=\"audience-hours-table\""));
        assert!(html.contains("data-testid=\"audience-observed-table\""));
        assert_eq!(html.matches("data-observed-hour=").count(), 168);
        assert!(html.contains("audience-sources-current"));
    }
    let malformed = call(&app, "/admin/audience?days=invalid", "").await;
    private(&malformed, StatusCode::UNAUTHORIZED);

    // Only the hourly relation is blocked. Authentication and C/F still work.
    let lock = LockedHours::acquire(&url);
    let failed = call(&app, "/admin/audience?days=7", &cookies[0]).await;
    private(&failed, StatusCode::SERVICE_UNAVAILABLE);
    let html = body(failed).await;
    assert!(html.contains("audience-hours-unavailable"));
    assert!(html.contains("data-testid=\"audience-observed-current\""));
    assert!(!html.contains("audience-hours-current"));
    assert!(html.contains("audience-sources-current"));
    pg(&url, &format!("UPDATE account_roles SET is_active=false WHERE user_id={admin} AND role='administrator'"));
    let revoked = call(&app, "/admin/audience?days=7", &cookies[0]).await;
    private(&revoked, StatusCode::FORBIDDEN);
    assert!(!body(revoked).await.contains("audience-hours"));
    drop(lock);
    pg(&url, &format!("UPDATE account_roles SET is_active=true WHERE user_id={admin} AND role='administrator'"));
    accounts
        .revoke_session(&session_token_digest(&tokens[0]))
        .await
        .unwrap();
    private(
        &call(&app, "/admin/audience", &cookies[0]).await,
        StatusCode::UNAUTHORIZED,
    );
    drop(fixtures);
    pool.close().await;
    private(
        &call(&app, "/admin/audience", &cookies[0]).await,
        StatusCode::SERVICE_UNAVAILABLE,
    );
    pg(&url, &format!("BEGIN; DELETE FROM account_roles WHERE user_id IN({admin},{reader},{moderator}); DELETE FROM users WHERE id IN({admin},{reader},{moderator}); COMMIT"));
}

#[path = "audience_hours_http_pg/observed.rs"]
mod observed;
