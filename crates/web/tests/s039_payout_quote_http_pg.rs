//! S039: admin read-only payout quote via disposable PostgreSQL HTTP.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
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
use tor_commerce::{
    domain::money::IdempotencyKey,
    repositories::creator_finance::PgCreatorFinance,
};
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn pg(url: &str, sql: &str) -> String {
    assert!(url.ends_with("/tor_ci"), "never use a retained database");
    let out = Command::new("psql")
        .env("PGOPTIONS", "-c statement_timeout=15000 -c lock_timeout=5000")
        .args(["-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1", "--dbname", url, "--command", sql])
        .output()
        .expect("psql required for HTTP acceptance");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn app(accounts: PgAccounts, catalog: PgCatalog) -> Router {
    let reader = PgReader::new(catalog.pool().clone());
    tor_web::router_with_postgres_reading_auth(catalog, reader, accounts, false, ORIGIN.to_owned())
}

async fn get(site: &Router, path: &str, cookie: &str) -> axum::response::Response {
    let req = Request::builder()
        .uri(path)
        .method("GET")
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    site.clone().oneshot(req).await.unwrap()
}

#[tokio::test]
#[ignore = "requires disposable tor_ci and quote flag disabled"]
async fn s039_quote_http_disabled_by_default() {
    assert_ne!(
        std::env::var("TOR_S039_PAYOUT_QUOTE_HTTP_ENABLED").as_deref(),
        Ok("1")
    );
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool().clone());
    let site = app(accounts, catalog);
    assert_eq!(
        get(&site, "/admin/finance/payouts/1/quote", "").await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci and explicit quote flag"]
async fn s039_quote_http_admin_only_and_no_wallet_movement() {
    assert_eq!(
        std::env::var("TOR_S039_PAYOUT_QUOTE_HTTP_ENABLED").as_deref(),
        Ok("1")
    );
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let owner_token = generate_session_token();
    let admin_token = generate_session_token();
    let csrf = generate_session_token();
    let owner = accounts.register_with_session(
        &format!("s039-quote-http-owner-{stamp}@example.invalid"),
        "Payout owner",
        &hash_password("S039QuoteOwnerPassword2026").unwrap(),
        &session_token_digest(&owner_token),
        NonZeroU32::new(3600).unwrap(),
    ).await.unwrap();
    let admin = accounts.register_with_session(
        &format!("s039-quote-http-admin-{stamp}@example.invalid"),
        "Payout admin",
        &hash_password("S039QuoteAdministratorPassword2026").unwrap(),
        &session_token_digest(&admin_token),
        NonZeroU32::new(3600).unwrap(),
    ).await.unwrap();
    pg(&url, &format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:s039:quote:http:{stamp}','ru','S039 quote HTTP','public',{owner})"
    ));
    pg(&url, &format!(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES({owner},1000,'RUB')"
    ));
    pg(&url, &format!(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES({admin},'administrator',true)"
    ));
    let finance = PgCreatorFinance::new(pool.clone());
    let id = finance.reserve_once(
        owner, 600, &IdempotencyKey::parse(&format!("{}8", "Q".repeat(42))).unwrap(),
    ).await.unwrap().id;
    finance.reserve_once(
        owner, 300, &IdempotencyKey::parse(&format!("{}8", "R".repeat(42))).unwrap(),
    ).await.unwrap();
    let site = app(accounts, catalog);
    let uri = format!("/admin/finance/payouts/{id}/quote");
    let owner_cookie = format!("tor-dev-session={owner_token}; tor-dev-csrf={csrf}");
    let admin_cookie = format!("tor-dev-session={admin_token}; tor-dev-csrf={csrf}");
    assert_eq!(get(&site, &uri, "").await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(get(&site, &uri, &owner_cookie).await.status(), StatusCode::NOT_FOUND);
    let res = get(&site, &uri, &admin_cookie).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(res.headers()["x-robots-tag"], "noindex, nofollow");
    let body = to_bytes(res.into_body(), 128 * 1024).await.unwrap();
    let markup = String::from_utf8(body.to_vec()).unwrap();
    assert!(markup.contains(&format!("Проверка заявки №{id}")));
    assert!(markup.contains("Резерв других заявок: 300 коп."));
    assert!(markup.contains("Доступно без резерва этой заявки: 700 коп."));
    assert!(markup.contains("не подтверждение"));
    assert!(!markup.contains("<form"));
    assert_eq!(pg(&url, &format!(
        "SELECT count(*) FROM finance_creator_payout_approval_read_audit
         WHERE admin_user_id={admin} AND payout_request_id={id}"
    )), "1");
    assert_eq!(pg(&url, &format!(
        "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={owner}"
    )), "1000");
    assert_eq!(pg(&url, &format!(
        "SELECT count(*) FROM reader_wallet_entries WHERE user_id={owner}"
    )), "0");
    assert_eq!(pg(&url, &format!(
        "SELECT state FROM finance_creator_payout_requests WHERE id={id}"
    )), "pending");
    pg(&url, &format!(
        "UPDATE account_roles SET is_active=false
         WHERE user_id={admin} AND role='administrator'"
    ));
    assert_eq!(get(&site, &uri, &admin_cookie).await.status(), StatusCode::NOT_FOUND);
    pg(&url, &format!(
        "UPDATE sessions SET revoked_at=COALESCE(revoked_at,clock_timestamp())
         WHERE user_id={admin}"
    ));
    pool.close().await;
}
