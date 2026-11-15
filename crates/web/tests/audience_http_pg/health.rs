use super::*;
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, ChildStdin, Stdio},
    time::{Duration, Instant},
};

struct HealthFixture {
    app: Router,
    catalog: PgCatalog,
    public: PgCatalog,
    url: String,
    owner_url: String,
    schema: String,
    admin: i64,
    token: String,
    cookie: String,
}

impl HealthFixture {
    async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL").unwrap();
        let owner_url = std::env::var("MIGRATOR_URL").unwrap();
        for value in [&url, &owner_url] {
            assert!(value.ends_with("/tor_ci") && value.contains("127.0.0.1:5432"));
        }
        let public = PgCatalog::connect(&url).await.unwrap();
        let accounts = PgAccounts::from_pool(public.pool());
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let schema = format!("html_health_http_{stamp}");
        let token = generate_session_token();
        let password = hash_password("HtmlHealthHttpFixture2026").unwrap();
        let admin = accounts
            .register_with_session(
                &format!("html-health-{stamp}@example.invalid"),
                "Health fixture",
                &password,
                &session_token_digest(&token),
                NonZeroU32::new(3600).unwrap(),
            )
            .await
            .unwrap();
        pg(
            &url,
            &format!("INSERT INTO account_roles(user_id,role) VALUES({admin},'administrator')"),
        );
        pg(&owner_url, &format!("CREATE SCHEMA {schema}"));
        // Existing owner tables stay real, through readonly views. Only this
        // fixture's H capability/counts differ. No canonical marker is changed.
        let relations = pg(
            &owner_url,
            "SELECT c.relname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace \
             WHERE n.nspname='public' AND c.relkind IN('r','p','v','m') ORDER BY c.relname",
        );
        for name in relations.lines() {
            if [
                "_sqlx_migrations",
                "ril_flow_observed_hourly",
                "ril_flow_observed_receipts",
                "ril_flow_health_epochs",
                "ril_flow_health_hourly",
            ]
            .contains(&name)
            {
                continue;
            }
            assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
            pg(
                &owner_url,
                &format!("CREATE VIEW {schema}.\"{name}\" AS SELECT * FROM public.\"{name}\""),
            );
        }
        for name in [
            "_sqlx_migrations",
            "ril_flow_observed_hourly",
            "ril_flow_observed_receipts",
        ] {
            pg(
                &owner_url,
                &format!("CREATE TABLE {schema}.{name} (LIKE public.{name} INCLUDING ALL)"),
            );
        }
        pg(&owner_url, &format!(
            "INSERT INTO {schema}._sqlx_migrations SELECT * FROM public._sqlx_migrations WHERE version<>202611294685;
             GRANT USAGE ON SCHEMA {schema} TO tor_app;
             GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO tor_app"
        ));
        // Exercise the real three-connection runtime pool. The isolated schema
        // must not fall back to public when its optional H tables are absent.
        let isolated = format!(
            "{url}?options=-csearch_path%3D{schema}%2Cpg_catalog%20-cstatement_timeout%3D2000%20-capplication_name%3D{schema}"
        );
        let catalog = PgCatalog::connect(&isolated).await.unwrap();
        let app = tor_web::router_with_postgres_reading_auth(
            catalog.clone(),
            PgReader::new(catalog.pool()),
            PgAccounts::from_pool(catalog.pool()),
            false,
            ORIGIN.to_owned(),
        );
        let cookie = format!("tor-dev-session={token}");
        Self {
            app,
            catalog,
            public,
            url,
            owner_url,
            schema,
            admin,
            token,
            cookie,
        }
    }

    fn owner(&self, sql: &str) -> String {
        pg(&self.owner_url, &sql.replace("{{schema}}", &self.schema))
    }

    async fn get(&self) -> (StatusCode, String) {
        let response = call(
            &self.app,
            "GET",
            "/admin/audience?days=1",
            &self.cookie,
            "",
            "",
        )
        .await;
        private(&response);
        assert!(response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("script-src 'none'"));
        let status = response.status();
        (status, page(response).await)
    }

    fn create_health(&self) {
        for name in ["ril_flow_health_epochs", "ril_flow_health_hourly"] {
            self.owner(&format!(
                "CREATE TABLE {{{{schema}}}}.{name} (LIKE public.{name} INCLUDING ALL);
                 GRANT SELECT ON {{{{schema}}}}.{name} TO tor_app"
            ));
        }
    }

    async fn finish(self) {
        self.catalog.close_for_test().await;
        pg(
            &self.owner_url,
            &format!("DROP SCHEMA {} CASCADE", self.schema),
        );
        pg(
            &self.url,
            &format!(
                "DELETE FROM account_roles WHERE user_id={};
             DELETE FROM users WHERE id={}",
                self.admin, self.admin
            ),
        );
        self.public.close_for_test().await;
    }
}

struct HealthLock(Child, Option<ChildStdin>);
impl HealthLock {
    fn new(fixture: &HealthFixture) -> Self {
        let mut child = Command::new("psql")
            .args([
                "-XAtq",
                "-v",
                "ON_ERROR_STOP=1",
                "--dbname",
                &fixture.owner_url,
            ])
            .env(
                "PGOPTIONS",
                "-c statement_timeout=5000 -c idle_in_transaction_session_timeout=10000",
            )
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut input = child.stdin.take().unwrap();
        let mut output = BufReader::new(child.stdout.take().unwrap());
        writeln!(
            input,
            "BEGIN; LOCK TABLE {}.ril_flow_health_epochs IN ACCESS EXCLUSIVE MODE; SELECT 'READY';",
            fixture.schema
        )
        .unwrap();
        input.flush().unwrap();
        loop {
            let mut line = String::new();
            assert!(output.read_line(&mut line).unwrap() > 0);
            if line.trim() == "READY" {
                break;
            }
        }
        Self(child, Some(input))
    }
}
impl Drop for HealthLock {
    fn drop(&mut self) {
        if let Some(mut input) = self.1.take() {
            let _ = writeln!(input, "ROLLBACK;\n\\q");
            let _ = input.flush();
        }
        let _ = self.0.wait();
    }
}

fn observed_total<'a>(html: &'a str, period: &str) -> &'a str {
    let marker = format!("data-testid=\"audience-observed-{period}\"");
    html.split(marker.as_str())
        .nth(1)
        .unwrap()
        .split("</section>")
        .next()
        .unwrap()
        .split("data-metric=\"observed-total\">")
        .nth(1)
        .unwrap()
        .split("</strong>")
        .next()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci with HTML health migration"]
async fn html_health_http_preserves_legacy_counts_and_requires_durable_complete_zero() {
    let f = HealthFixture::new().await;
    let anonymous = call(&f.app, "GET", "/admin/audience?days=1", "", "", "").await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    private(&anonymous);
    assert!(!page(anonymous).await.contains("data-metric="));

    let anchor: i64 = pg(&f.url,
        "SELECT extract(epoch FROM date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')::bigint"
    ).parse().unwrap();
    f.owner(&format!(
        "INSERT INTO {{{{schema}}}}.ril_flow_observed_hourly VALUES(to_timestamp({}),0,3,7)",
        anchor - 3600
    ));
    let (status, html) = f.get().await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("audience-html-health-missing"));
    assert_eq!(observed_total(&html, "current"), "7");
    assert_eq!(observed_total(&html, "previous"), "Нет измерения");

    f.create_health();
    let (status, html) = f.get().await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "tables without an applied marker are damage"
    );
    assert!(html.contains("audience-html-health-unavailable"));
    assert!(!html.contains("data-metric=\"observed-total\">0"));

    f.owner("INSERT INTO {{schema}}._sqlx_migrations SELECT * FROM public._sqlx_migrations WHERE version=202611294685");
    let (status, html) = f.get().await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("data-health-state=\"unknown\""));
    assert_eq!(observed_total(&html, "previous"), "Нет измерения");
    let epoch: i64 = f
        .owner(&format!(
        "INSERT INTO {{{{schema}}}}.ril_flow_health_epochs(started_ms,confirmed_ms,lease_until_ms)
         VALUES({},{},{}) RETURNING epoch",
        (anchor - 172800) * 1000, anchor * 1000, anchor * 1000 + 30000
    ))
        .parse()
        .unwrap();
    let snapshot = "SELECT count(*),sum(generation)::bigint,sum(confirmed_ms)::bigint FROM {{schema}}.ril_flow_health_epochs";
    let before = f.owner(snapshot);
    let (status, html) = f.get().await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("data-health-period=\"current\" data-health-state=\"confirmed\""));
    assert!(html.contains("data-health-period=\"previous\" data-health-state=\"confirmed\""));
    assert_eq!(observed_total(&html, "current"), "7");
    assert_eq!(observed_total(&html, "previous"), "0");
    assert_eq!(
        before,
        f.owner(snapshot),
        "GET does not create or advance acknowledgements"
    );
    f.owner(&format!(
        "INSERT INTO {{{{schema}}}}.ril_flow_health_hourly(epoch,bucket_ms,had_loss) VALUES({epoch},{},true)",
        (anchor - 25 * 3600) * 1000
    ));
    let (status, html) = f.get().await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("data-health-period=\"previous\" data-health-state=\"unknown\""));
    assert_eq!(observed_total(&html, "previous"), "Нет измерения");

    let lock = HealthLock::new(&f);
    let started = Instant::now();
    let (status, html) = f.get().await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(started.elapsed() < Duration::from_millis(1500));
    assert!(html.contains("audience-html-health-unavailable"));
    assert_eq!(pg(&f.url, &format!(
        "SELECT count(*) FROM pg_stat_activity WHERE application_name='{}' AND wait_event_type='Lock'",
        f.schema
    )), "0", "SQL stops before the held table is unlocked");
    let accounts = PgAccounts::from_pool(f.catalog.pool());
    assert!(tokio::time::timeout(
        Duration::from_millis(500),
        accounts.session_actor(&session_token_digest(&f.token))
    )
    .await
    .expect("real shared pool retained cancelled H read")
    .expect("independent live authority lookup must work before unlock")
    .is_some());
    drop(lock);

    pg(
        &f.url,
        &format!(
            "UPDATE account_roles SET is_active=false WHERE user_id={} AND role='administrator'",
            f.admin
        ),
    );
    let denied = call(&f.app, "GET", "/admin/audience?days=1", &f.cookie, "", "").await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    private(&denied);
    assert!(!page(denied).await.contains("audience-durable-quality"));
    pg(
        &f.url,
        &format!(
            "UPDATE account_roles SET is_active=true WHERE user_id={} AND role='administrator'",
            f.admin
        ),
    );
    PgAccounts::from_pool(f.public.pool())
        .revoke_session(&session_token_digest(&f.token))
        .await
        .unwrap();
    let denied = call(&f.app, "GET", "/admin/audience?days=1", &f.cookie, "", "").await;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    private(&denied);
    f.finish().await;
}
