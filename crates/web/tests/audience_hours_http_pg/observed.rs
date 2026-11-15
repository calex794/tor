//! Same live administrator route, no new consent or user-tracking endpoint.
use super::*;
use tor_traffic_behavior::services::observed_audience::ObservedAudienceReport;

struct ObservedFixtures {
    url: String,
    predicate: String,
    saved: String,
}
impl ObservedFixtures {
    fn install(url: &str, anchor: i64) -> Self {
        let recent = anchor - 3 * 3_600;
        let previous = anchor - 171 * 3_600;
        let old = anchor - 3_000 * 3_600;
        let predicate = format!(
            "bucket IN(to_timestamp({recent}),to_timestamp({previous}),to_timestamp({old}))"
        );
        let saved=pg(url,&format!("SELECT extract(epoch FROM bucket)::bigint,surface_code,acquisition_hint,html_requests FROM ril_flow_observed_hourly WHERE {predicate}"));
        let fixture = Self {
            url: url.into(),
            predicate,
            saved,
        };
        pg(
            url,
            &format!(
                "BEGIN; DELETE FROM ril_flow_observed_hourly WHERE {};
            INSERT INTO ril_flow_observed_hourly VALUES
            (to_timestamp({recent}),0,0,2),(to_timestamp({recent}),0,1,3),
            (to_timestamp({recent}),1,3,0),(to_timestamp({previous}),2,3,7),
            (to_timestamp({old}),3,3,11); COMMIT",
                fixture.predicate
            ),
        );
        fixture
    }
}
impl Drop for ObservedFixtures {
    fn drop(&mut self) {
        let mut sql = format!(
            "BEGIN; DELETE FROM ril_flow_observed_hourly WHERE {};",
            self.predicate
        );
        for row in self.saved.lines() {
            let row: Vec<i64> = row.split('|').map(|v| v.parse().unwrap()).collect();
            assert_eq!(row.len(), 4);
            sql.push_str(&format!(
                "INSERT INTO ril_flow_observed_hourly VALUES(to_timestamp({}),{},{},{});",
                row[0], row[1], row[2], row[3]
            ));
        }
        sql.push_str("COMMIT");
        pg(&self.url, &sql);
    }
}
struct Users {
    url: String,
    ids: Vec<i64>,
}
impl Drop for Users {
    fn drop(&mut self) {
        for id in &self.ids {
            pg(&self.url,&format!("BEGIN; DELETE FROM account_roles WHERE user_id={id}; DELETE FROM users WHERE id={id}; COMMIT"));
        }
    }
}
struct LockedObserved {
    child: Child,
    input: ChildStdin,
}
impl LockedObserved {
    fn acquire(url: &str) -> Self {
        let mut child = client(url)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        writeln!(input,"BEGIN; LOCK TABLE ril_flow_observed_hourly IN ACCESS EXCLUSIVE MODE; SELECT 'observed-locked';").unwrap();
        input.flush().unwrap();
        let mut ready = String::new();
        BufReader::new(child.stdout.take().unwrap())
            .read_line(&mut ready)
            .unwrap();
        assert_eq!(ready.trim(), "observed-locked");
        Self { child, input }
    }
}
impl Drop for LockedObserved {
    fn drop(&mut self) {
        let _ = writeln!(self.input, "ROLLBACK;\n\\q");
        let _ = self.input.flush();
        let _ = self.child.wait();
    }
}

fn matches_report(html: &str, report: &ObservedAudienceReport) {
    for (key, period) in [("current", &report.current), ("previous", &report.previous)] {
        let marker = format!("data-testid=\"audience-observed-{key}\"");
        let section = html
            .split(&marker)
            .nth(1)
            .unwrap()
            .split("</section>")
            .next()
            .unwrap();
        let actual = metric(section, "observed-total");
        match period.saved_total {
            Some(expected) => assert_eq!(actual, expected.to_string()),
            None => assert!(actual.parse::<i64>().is_err()),
        }
        assert_eq!(
            metric(section, "observed-hours"),
            period.recorded_hours.to_string()
        );
    }
    assert_eq!(
        html.matches("data-observed-hour=").count(),
        report.current.hours.len().min(168)
    );
    assert!(html.contains(&format!("value=\"{}\" selected", report.time_zone)));
    assert!(html.contains("audience-observed-week"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn observed_hours_guarded_periods_timezone_timeout_and_recovery() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let repo = PgFlowAudience::new(pool.clone());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("ObservedHoursPassword2026").unwrap();
    let mut users = Users {
        url: url.clone(),
        ids: vec![],
    };
    let mut cookies = Vec::new();
    for kind in ["admin", "reader"] {
        let token = generate_session_token();
        let id = accounts
            .register_with_session(
                &format!("observed-{kind}-{stamp}@example.invalid"),
                "Clock fixture",
                &password,
                &session_token_digest(&token),
                NonZeroU32::new(3600).unwrap(),
            )
            .await
            .unwrap();
        users.ids.push(id);
        cookies.push(format!("tor-dev-session={token}"));
    }
    let admin = users.ids[0];
    pg(
        &url,
        &format!("INSERT INTO account_roles(user_id,role) VALUES({admin},'administrator')"),
    );
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.into(),
    );
    let anchor = repo
        .observed_report(7, "UTC")
        .await
        .unwrap()
        .current
        .until_epoch;
    let fixtures = ObservedFixtures::install(&url, anchor);
    private(
        &call(&app, "/admin/audience?time_zone=invalid", "").await,
        StatusCode::UNAUTHORIZED,
    );
    private(
        &call(&app, "/admin/audience?time_zone=UTC", &cookies[1]).await,
        StatusCode::FORBIDDEN,
    );
    for days in [1, 7, 30, 90] {
        let report = repo.observed_report(days, "Europe/Moscow").await.unwrap();
        let response = call(
            &app,
            &format!("/admin/audience?days={days}&time_zone=Europe%2FMoscow"),
            &cookies[0],
        )
        .await;
        private(&response, StatusCode::OK);
        let html = body(response).await;
        matches_report(&html, &report);
        assert!(!html.contains("example.invalid"));
        assert!(!html.contains("Clock fixture"));
    }
    for path in [
        "/admin/audience?time_zone=unknown",
        "/admin/audience?time_zone=UTC&time_zone=Europe/Moscow",
        "/admin/audience?time_zone=%3Cscript%3E",
        "/admin/audience?days=0",
    ] {
        private(
            &call(&app, path, &cookies[0]).await,
            StatusCode::BAD_REQUEST,
        );
    }
    let lock = LockedObserved::acquire(&url);
    let response = call(
        &app,
        "/admin/audience?days=7&time_zone=Europe/Moscow",
        &cookies[0],
    )
    .await;
    private(&response, StatusCode::SERVICE_UNAVAILABLE);
    let html = body(response).await;
    assert!(html.contains("audience-observed-unavailable"));
    assert!(!html.contains("audience-observed-current"));
    assert!(html.contains("audience-sources-current"));
    assert!(html.contains("value=\"Europe/Moscow\" selected"));
    drop(lock);
    private(
        &call(&app, "/admin/audience?days=90", &cookies[0]).await,
        StatusCode::OK,
    );
    pg(&url,&format!("UPDATE account_roles SET is_active=false WHERE user_id={admin} AND role='administrator'"));
    private(
        &call(&app, "/admin/audience", &cookies[0]).await,
        StatusCode::FORBIDDEN,
    );
    drop(fixtures);
    drop(users);
    pool.close().await;
}
