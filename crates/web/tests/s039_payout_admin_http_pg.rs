//! S039: real administrator HTTP path with disposable tor_ci only.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use std::{num::NonZeroU32, process::Command, time::{SystemTime, UNIX_EPOCH}};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_commerce::{
    domain::money::IdempotencyKey,
    repositories::creator_finance::PgCreatorFinance,
};
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";
const PATH: &str = "/admin/finance/payouts";

fn pg(url: &str, sql: &str) -> String {
    assert!(url.ends_with("/tor_ci"), "never use retained DB");
    let output = Command::new("psql")
        .env("PGOPTIONS","-c statement_timeout=15000 -c lock_timeout=5000")
        .args(["-X","-A","-t","-q","-v","ON_ERROR_STOP=1",
            "--dbname",url,"--command",sql])
        .output().expect("psql required for finance HTTP acceptance");
    assert!(output.status.success(),
        "fixture SQL failed: {}",String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn request(site: &Router, method: &str, path: &str,
                 cookie: &str, origin: &str, form: &str) -> axum::response::Response {
    let req = Request::builder().uri(path).method(method)
        .header(header::COOKIE,cookie)
        .header(header::ORIGIN,origin)
        .header(header::CONTENT_TYPE,"application/x-www-form-urlencoded")
        .body(Body::from(form.to_owned())).unwrap();
    site.clone().oneshot(req).await.unwrap()
}

async fn html(response: axum::response::Response) -> String {
    let bytes=to_bytes(response.into_body(),128*1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn app(accounts: PgAccounts, catalog: PgCatalog) -> Router {
    let reader = PgReader::new(catalog.pool().clone());
    tor_web::router_with_postgres_reading_auth(
        catalog,reader,accounts,false,ORIGIN.to_owned()
    )
}

#[tokio::test]
#[ignore = "requires disposable tor_ci with S039 HTTP flag disabled"]
async fn s039_http_disabled_by_default() {
    assert_ne!(std::env::var("TOR_S039_PAYOUT_REJECT_HTTP_ENABLED").as_deref(),Ok("1"));
    let url=std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"));
    let catalog=PgCatalog::connect(&url).await.unwrap();
    let accounts=PgAccounts::from_pool(catalog.pool().clone());
    let site=app(accounts,catalog);
    assert_eq!(request(&site,"POST","/admin/finance/payouts/1/reject",
        "",ORIGIN,"invalid=form").await.status(),StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci with explicit S039 rejection opt-in"]
async fn s039_http_admin_role_csrf_idempotent_rejection() {
    assert_eq!(std::env::var("TOR_S039_PAYOUT_REJECT_HTTP_ENABLED").as_deref(),Ok("1"));
    let url=std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"));
    let catalog=PgCatalog::connect(&url).await.unwrap();
    let pool=catalog.pool().clone();
    let accounts=PgAccounts::from_pool(pool.clone());
    let stamp=SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let applicant_token=generate_session_token();
    let admin_token=generate_session_token();
    let csrf=generate_session_token();
    let applicant=accounts.register_with_session(
        &format!("s039-http-applicant-{stamp}@example.invalid"),
        "Payout applicant",
        &hash_password("S039PayoutApplicantPassword2026").unwrap(),
        &session_token_digest(&applicant_token),
        NonZeroU32::new(3600).unwrap(),
    ).await.unwrap();
    let admin=accounts.register_with_session(
        &format!("s039-http-admin-{stamp}@example.invalid"),
        "Payout administrator",
        &hash_password("S039PayoutAdministratorPassword2026").unwrap(),
        &session_token_digest(&admin_token),
        NonZeroU32::new(3600).unwrap(),
    ).await.unwrap();
    pg(&url,&format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:s039:http:{stamp}','ru','S039 finance','public',{applicant})"
    ));
    pg(&url,&format!(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES({applicant},1000,'RUB')"
    ));
    pg(&url,&format!(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES({admin},'administrator',true)
         ON CONFLICT(user_id,role) DO UPDATE SET is_active=true"
    ));
    let key=IdempotencyKey::parse(&format!("{}9","H".repeat(42))).unwrap();
    let receipt=PgCreatorFinance::new(pool.clone())
        .reserve_once(applicant,600,&key).await.unwrap();
    let id=receipt.id;
    let site=app(accounts,catalog);
    let applicant_cookie=format!("tor-dev-session={applicant_token}; tor-dev-csrf={csrf}");
    let admin_cookie=format!("tor-dev-session={admin_token}; tor-dev-csrf={csrf}");
    assert_eq!(request(&site,"GET",PATH,"",ORIGIN,"").await.status(),
        StatusCode::UNAUTHORIZED);
    assert_eq!(request(&site,"GET",PATH,&applicant_cookie,ORIGIN,"").await.status(),
        StatusCode::NOT_FOUND);
    assert_eq!(pg(&url,&format!(
        "SELECT count(*) FROM finance_creator_payout_admin_audit WHERE admin_user_id={applicant}"
    )),"0");
    let page=request(&site,"GET",PATH,&admin_cookie,ORIGIN,"").await;
    assert_eq!(page.status(),StatusCode::OK);
    assert_eq!(page.headers()[header::CACHE_CONTROL],"no-store");
    assert_eq!(page.headers()["x-robots-tag"],"noindex, nofollow");
    let markup=html(page).await;
    assert!(markup.contains(&format!("Заявка №{id}")));
    assert!(markup.contains(&format!("action=\"{PATH}/{id}/reject\"")));
    assert!(!markup.contains("Подтвердить выплату"));
    let path=format!("{PATH}/{id}/reject");
    let form=format!("csrf_token={csrf}&reason=Declined+by+administrator");
    assert_eq!(request(&site,"POST",&path,&admin_cookie,
        "https://evil.invalid",&form).await.status(),StatusCode::FORBIDDEN);
    assert_eq!(request(&site,"POST",&path,&admin_cookie,
        ORIGIN,"csrf_token=wrong&reason=Declined").await.status(),StatusCode::FORBIDDEN);
    assert_eq!(request(&site,"POST",&path,&applicant_cookie,ORIGIN,&form)
        .await.status(),StatusCode::NOT_FOUND);
    assert_eq!(pg(&url,&format!(
        "SELECT state FROM finance_creator_payout_requests WHERE id={id}"
    )),"pending");
    let accepted=request(&site,"POST",&path,&admin_cookie,ORIGIN,&form).await;
    assert_eq!(accepted.status(),StatusCode::SEE_OTHER);
    assert_eq!(pg(&url,&format!(
        "SELECT state FROM finance_creator_payout_requests WHERE id={id}"
    )),"rejected");
    assert_eq!(pg(&url,&format!(
        "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={applicant}"
    )),"1000");
    assert_eq!(PgCreatorFinance::new(pool.clone()).totals(applicant)
        .await.unwrap().reserved_minor,0);
    let replay=request(&site,"POST",&path,&admin_cookie,ORIGIN,&form).await;
    assert_eq!(replay.status(),StatusCode::SEE_OTHER);
    assert_eq!(request(&site,"POST",&path,&admin_cookie,ORIGIN,
        &format!("csrf_token={csrf}&reason=Changed")).await.status(),
        StatusCode::CONFLICT);
    assert_eq!(pg(&url,&format!(
        "SELECT count(*) FROM finance_creator_payout_admin_audit
         WHERE payout_request_id={id} AND action='reject'"
    )),"1");
    pg(&url,&format!(
        "UPDATE account_roles SET is_active=false
         WHERE user_id={admin} AND role='administrator'"
    ));
    assert_eq!(request(&site,"GET",PATH,&admin_cookie,ORIGIN,"").await.status(),
        StatusCode::NOT_FOUND);
    pool.close().await;
}
