use super::*;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::time::{Duration, Instant};

struct HealthFixture {
    app: Router,
    pool: PgPool,
    owner: PgPool,
    public: PgPool,
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
        let public = PgPool::connect(&url).await.unwrap();
        let owner = PgPool::connect(&owner_url).await.unwrap();
        let accounts = PgAccounts::from_pool(public.clone());
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let schema = format!("html_health_http_{stamp}");
        let token = generate_session_token();
        let password = hash_password("HtmlHealthHttpFixture2026").unwrap();
        let admin = accounts.register_with_session(
            &format!("html-health-{stamp}@example.invalid"),
            "Health fixture",
            &password,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        ).await.unwrap();
        sqlx::query("INSERT INTO account_roles(user_id,role) VALUES($1,'administrator')")
            .bind(admin).execute(&public).await.unwrap();

        sqlx::query(&format!("CREATE SCHEMA {schema}")).execute(&owner).await.unwrap();
        // Other source owners and live A authorization still read their real
        // tables through views. Only this fixture's H capability/counts differ;
        // no canonical production table or migration marker is renamed/dropped.
        let relations: Vec<String> = sqlx::query_scalar(
            "SELECT c.relname FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace \
             WHERE n.nspname='public' AND c.relkind IN('r','p','v','m') ORDER BY c.relname")
            .fetch_all(&owner).await.unwrap();
        for name in relations {
            if [
                "_sqlx_migrations",
                "ril_flow_observed_hourly",
                "ril_flow_observed_receipts",
                "ril_flow_health_epochs",
                "ril_flow_health_hourly",
            ].contains(&name.as_str()) {
                continue;
            }
            assert!(name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'));
            sqlx::query(&format!("CREATE VIEW {schema}.\"{name}\" AS SELECT * FROM public.\"{name}\""))
                .execute(&owner).await.unwrap();
        }
        for name in [
            "_sqlx_migrations",
            "ril_flow_observed_hourly",
            "ril_flow_observed_receipts",
        ] {
            sqlx::query(&format!("CREATE TABLE {schema}.{name} (LIKE public.{name} INCLUDING ALL)"))
                .execute(&owner).await.unwrap();
        }
        sqlx::query(&format!(
            "INSERT INTO {schema}._sqlx_migrations SELECT * FROM public._sqlx_migrations WHERE version<>202611294685"))
            .execute(&owner).await.unwrap();
        for statement in [
            format!("GRANT USAGE ON SCHEMA {schema} TO tor_app"),
            format!("GRANT SELECT ON ALL TABLES IN SCHEMA {schema} TO tor_app"),
        ] {
            sqlx::query(&statement).execute(&owner).await.unwrap();
        }
        let namespace = schema.clone();
        let pool = PgPoolOptions::new().max_connections(8)
            .acquire_timeout(Duration::from_secs(2))
            .after_connect(move |connection, _| {
                let schema = namespace.clone();
                Box::pin(async move {
                    sqlx::query(&format!("SET search_path={schema},pg_catalog"))
                        .execute(&mut *connection).await?;
                    sqlx::query("SET statement_timeout='2s'")
                        .execute(&mut *connection).await?;
                    sqlx::query(&format!("SET application_name='{schema}'"))
                        .execute(connection).await?;
                    Ok(())
                })
            }).connect(&url).await.unwrap();
        let app = tor_web::router_with_postgres_reading_auth(
            PgCatalog::new(pool.clone()),
            PgReader::new(pool.clone()),
            PgAccounts::from_pool(pool.clone()),
            false,
            ORIGIN.to_owned(),
        );
        let cookie = format!("tor-dev-session={token}");
        Self { app, pool, owner, public, schema, admin, token, cookie }
    }

    async fn get(&self) -> (StatusCode, String) {
        let response = call(
            &self.app, "GET", "/admin/audience?days=1", &self.cookie, "", "",
        ).await;
        private(&response);
        assert!(response.headers()["content-security-policy"].to_str().unwrap()
            .contains("script-src 'none'"));
        let status = response.status();
        (status, page(response).await)
    }

    async fn create_health(&self) {
        for name in ["ril_flow_health_epochs", "ril_flow_health_hourly"] {
            sqlx::query(&format!("CREATE TABLE {}.{name} (LIKE public.{name} INCLUDING ALL)", self.schema))
                .execute(&self.owner).await.unwrap();
            sqlx::query(&format!("GRANT SELECT ON {}.{name} TO tor_app", self.schema))
                .execute(&self.owner).await.unwrap();
        }
    }

    async fn finish(self) {
        self.pool.close().await;
        sqlx::query(&format!("DROP SCHEMA {} CASCADE", self.schema))
            .execute(&self.owner).await.unwrap();
        sqlx::query("DELETE FROM account_roles WHERE user_id=$1")
            .bind(self.admin).execute(&self.public).await.unwrap();
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(self.admin).execute(&self.public).await.unwrap();
        self.public.close().await;
        self.owner.close().await;
    }
}

fn observed_total<'a>(html: &'a str, period: &str) -> &'a str {
    let marker = format!("data-testid=\"audience-observed-{period}\"");
    html.split(marker.as_str()).nth(1).unwrap()
        .split("</section>").next().unwrap()
        .split("data-metric=\"observed-total\">").nth(1).unwrap()
        .split("</strong>").next().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL 18 tor_ci with HTML health migration"]
async fn html_health_http_preserves_legacy_counts_and_requires_durable_complete_zero() {
    let f = HealthFixture::new().await;
    let anonymous = call(&f.app, "GET", "/admin/audience?days=1", "", "", "").await;
    assert_eq!(anonymous.status(), StatusCode::UNAUTHORIZED);
    private(&anonymous);
    assert!(!page(anonymous).await.contains("data-metric="));

    let anchor: i64 = sqlx::query_scalar(
        "SELECT extract(epoch FROM date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC')::bigint")
        .fetch_one(&f.pool).await.unwrap();
    sqlx::query(&format!("INSERT INTO {}.ril_flow_observed_hourly VALUES(to_timestamp($1::bigint),0,3,7)", f.schema))
        .bind(anchor - 3600).execute(&f.owner).await.unwrap();
    let (status, html) = f.get().await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("audience-html-health-missing"));
    assert_eq!(observed_total(&html, "current"), "7");
    assert_eq!(observed_total(&html, "previous"), "Нет измерения");

    f.create_health().await;
    let (status, html) = f.get().await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "tables without an applied marker are damage");
    assert!(html.contains("audience-html-health-unavailable"));
    assert!(!html.contains("data-metric=\"observed-total\">0"));

    sqlx::query(&format!("INSERT INTO {}._sqlx_migrations SELECT * FROM public._sqlx_migrations WHERE version=202611294685", f.schema))
        .execute(&f.owner).await.unwrap();
    let (status, html) = f.get().await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("data-health-state=\"unknown\""));
    assert_eq!(observed_total(&html, "previous"), "Нет измерения");
    let epoch: i64 = sqlx::query_scalar(&format!(
        "INSERT INTO {}.ril_flow_health_epochs(started_ms,confirmed_ms,lease_until_ms) VALUES($1,$2,$2+30000) RETURNING epoch", f.schema))
        .bind((anchor - 172800) * 1000).bind(anchor * 1000)
        .fetch_one(&f.owner).await.unwrap();
    let before: (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*),sum(generation)::bigint,sum(confirmed_ms)::bigint FROM ril_flow_health_epochs")
        .fetch_one(&f.pool).await.unwrap();
    let (status, html) = f.get().await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("data-health-period=\"current\" data-health-state=\"confirmed\""));
    assert!(html.contains("data-health-period=\"previous\" data-health-state=\"confirmed\""));
    assert_eq!(observed_total(&html, "current"), "7");
    assert_eq!(observed_total(&html, "previous"), "0");
    let after: (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*),sum(generation)::bigint,sum(confirmed_ms)::bigint FROM ril_flow_health_epochs")
        .fetch_one(&f.pool).await.unwrap();
    assert_eq!(before, after, "GET does not create or advance acknowledgements");
    sqlx::query(&format!("INSERT INTO {}.ril_flow_health_hourly(epoch,bucket_ms,had_loss) VALUES($1,$2,true)", f.schema))
        .bind(epoch).bind((anchor - 25 * 3600) * 1000)
        .execute(&f.owner).await.unwrap();
    let (status, html) = f.get().await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("data-health-period=\"previous\" data-health-state=\"unknown\""));
    assert_eq!(observed_total(&html, "previous"), "Нет измерения");

    let mut lock = f.owner.begin().await.unwrap();
    sqlx::query(&format!("LOCK TABLE {}.ril_flow_health_epochs IN ACCESS EXCLUSIVE MODE", f.schema))
        .execute(&mut *lock).await.unwrap();
    let started = Instant::now();
    let (status, html) = f.get().await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(started.elapsed() < Duration::from_millis(1500));
    assert!(html.contains("audience-html-health-unavailable"));
    let waiting: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM pg_stat_activity WHERE application_name=$1 AND wait_event_type='Lock'")
        .bind(&f.schema).fetch_one(&f.pool).await.unwrap();
    assert_eq!(waiting, 0, "SQL lock timeout releases the failed read before the lock is removed");
    assert_eq!(sqlx::query_scalar::<_,i32>("SELECT 1").fetch_one(&f.pool).await.unwrap(), 1);
    lock.rollback().await.unwrap();

    sqlx::query("UPDATE account_roles SET is_active=false WHERE user_id=$1 AND role='administrator'")
        .bind(f.admin).execute(&f.public).await.unwrap();
    let denied = call(&f.app, "GET", "/admin/audience?days=1", &f.cookie, "", "").await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    private(&denied);
    assert!(!page(denied).await.contains("audience-durable-quality"));
    sqlx::query("UPDATE account_roles SET is_active=true WHERE user_id=$1 AND role='administrator'")
        .bind(f.admin).execute(&f.public).await.unwrap();
    PgAccounts::from_pool(f.public.clone()).revoke_session(&session_token_digest(&f.token))
        .await.unwrap();
    let denied = call(&f.app, "GET", "/admin/audience?days=1", &f.cookie, "", "").await;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    private(&denied);
    f.finish().await;
}
