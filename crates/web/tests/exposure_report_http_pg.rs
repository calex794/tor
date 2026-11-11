//! Administrative reports against current A/F PostgreSQL sources.
//! SQL is fixture setup via psql; the web package has no SQLx dependency.
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::{
    num::NonZeroU32,
    process::Command,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_contracts::exposure::{
    CardObservation, EditorialObservationGuard, ObservationGuardError,
};
use tor_web::flow_exposure::Runtime;
use tower::ServiceExt;

const REPORT: &str = "/admin/audience/exposure";
const ORIGIN: &str = "http://127.0.0.1:3113";
struct Guard { book: i64 }
#[async_trait::async_trait]
impl EditorialObservationGuard for Guard {
    async fn allowed(&self, _: &CardObservation) -> Result<bool, ObservationGuardError> {
        Ok(false)
    }
    async fn campaign_book(&self, campaign: i64) -> Result<Option<i64>, ObservationGuardError> {
        Ok((campaign == 17).then_some(self.book))
    }
}
struct Fixture {
    app: Router,
    url: String,
    actor: i64,
    book: i64,
    cookie: String,
}
fn sql(url: &str, statement: &str) -> String {
    let result = Command::new("psql")
        .args(["-XAtq", "-v", "ON_ERROR_STOP=1", "--dbname", url, "-c", statement])
        .env("PGOPTIONS", "-c statement_timeout=5000 -c lock_timeout=2000")
        .output().unwrap();
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    String::from_utf8(result.stdout).unwrap().trim().into()
}
impl Fixture {
    async fn new() -> Self {
        let url = std::env::var("TEST_DATABASE_URL").expect("explicit disposable tor_ci");
        let port = std::env::var("FLOW_TEST_PG_PORT").unwrap_or_else(|_| "5432".into())
            .parse::<u16>().expect("explicit disposable PostgreSQL port");
        assert!(port > 0);
        assert!(url.starts_with("postgresql://tor_app:")
            && url.ends_with(&format!("@127.0.0.1:{port}/tor_ci")));
        let catalog = PgCatalog::connect(&url).await.unwrap();
        let accounts = PgAccounts::from_pool(catalog.pool().clone());
        let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let token = generate_session_token();
        let actor = accounts.register_with_session(
            &format!("flow-report-{stamp}@tor-ci.example"), "Report operator",
            "$argon2id$test", &session_token_digest(&token), NonZeroU32::new(3600).unwrap(),
        ).await.unwrap();
        let book = sql(&url, &format!("INSERT INTO books(source_identity,target_language,title,visibility,access_level,genres)
            VALUES('ci:flow-report:{stamp}','ru','Report <script>safe</script>','public','basic',ARRAY['zzz-low <script>']) RETURNING id")).parse().unwrap();
        let (runtime, worker) = Runtime::new(accounts, ORIGIN.into(), false, Some(Arc::new(Guard { book })));
        // A report read cannot start, certify, or mutate an observation epoch.
        // The unstarted worker leaves the empty durable history unknown.
        drop(worker);
        Self {
            app: runtime.attach(Router::new()), url, actor, book,
            cookie: format!("tor-dev-session={token}"),
        }
    }
    async fn get(&self, suffix: &str, cookie: &str) -> Response {
        self.app.clone().oneshot(Request::get(format!("{REPORT}{suffix}"))
            .header(header::COOKIE, cookie).body(Body::empty()).unwrap()).await.unwrap()
    }
    fn admin(&self) {
        sql(&self.url, &format!("INSERT INTO account_roles(user_id,role) VALUES({},'administrator')", self.actor));
    }
    fn cleanup(self) {
        sql(&self.url, &format!("DELETE FROM flow_observation_hourly WHERE book_id={};
            DELETE FROM books WHERE id={};DELETE FROM account_roles WHERE user_id={};
            DELETE FROM users WHERE id={}", self.book, self.book, self.actor, self.actor));
    }
}
async fn body(response: Response, status: StatusCode) -> String {
    assert_eq!(response.status(), status);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response.headers()["x-robots-tag"], "noindex, nofollow");
    assert!(!response.headers().contains_key(header::SET_COOKIE));
    String::from_utf8(to_bytes(response.into_body(), 2*1024*1024).await.unwrap().to_vec()).unwrap()
}
#[tokio::test(flavor="multi_thread", worker_threads=2)]
#[ignore="requires disposable PostgreSQL 18 with current migrations"]
async fn calendar_and_closed_views_keep_health_and_campaign_scopes_distinct() {
    let f = Fixture::new().await;
    body(f.get("?window=closed", "").await, StatusCode::UNAUTHORIZED).await;
    body(f.get("?window=closed", &f.cookie).await, StatusCode::FORBIDDEN).await;
    f.admin();
    sql(&f.url, &format!("INSERT INTO flow_observation_hourly(bucket,book_id,surface,campaign_id,slot,opportunities,rendered,views)
        VALUES(date_trunc('hour',now() AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'-interval '2 hours',{},'home',17,'home_featured',20,20,0)", f.book));
    let before = sql(&f.url, "SELECT count(*) FROM flow_observation_epochs");
    let query = "?days=7&source=editorial&campaign=17";
    let calendar = body(f.get(query, &f.cookie).await, StatusCode::OK).await;
    assert!(calendar.contains("календарных дней"));
    assert!(calendar.contains("data-testid=\"exposure-genres\""));
    assert!(!calendar.contains("data-testid=\"exposure-health\""));
    assert!(calendar.contains("data-metric=\"books\">1</span>"));
    let closed = body(f.get(&format!("{query}&window=closed"), &f.cookie).await, StatusCode::OK).await;
    assert!(closed.contains("data-testid=\"exposure-closed-window\""));
    assert!(closed.contains("data-testid=\"exposure-health-unknown\""));
    assert!(!closed.contains("data-testid=\"exposure-health-complete\""));
    assert!(closed.contains("data-metric=\"low-genres\">1</span>"));
    assert!(closed.contains("zzz-low &lt;script&gt;"));
    assert!(!closed.contains("<script>safe") && !closed.contains("zzz-low <script>"));
    assert!(closed.contains("window=closed&amp;source=editorial&amp;campaign=17"));
    assert!(!closed.contains("data-flow-proof"));
    body(f.get("?window=closed&campaign=18", &f.cookie).await, StatusCode::NOT_FOUND).await;
    body(f.get("?window=hour", &f.cookie).await, StatusCode::BAD_REQUEST).await;
    assert_eq!(sql(&f.url, "SELECT count(*) FROM flow_observation_epochs"), before);
    sql(&f.url, &format!("UPDATE books SET admin_hold=true WHERE id={}", f.book));
    let withdrawn = body(f.get(&format!("{query}&window=closed"), &f.cookie).await, StatusCode::OK).await;
    assert!(withdrawn.contains("data-metric=\"books\">0</span>"));
    assert!(withdrawn.contains("data-metric=\"genres\">0</span>"));
    sql(&f.url, &format!("DELETE FROM account_roles WHERE user_id={}", f.actor));
    body(f.get("?window=closed", &f.cookie).await, StatusCode::FORBIDDEN).await;
    f.cleanup();
}
