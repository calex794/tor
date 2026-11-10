//! G-09 / #748: operator browser gate, bounded controls and durable audit.
//! No real S3 client or production files are touched by this HTTP test.
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
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";
const BUCKET: &str = "tor-private-g09-operator-ci";
const PATH: &str = "/admin/storage/reconciliation";

fn pg(url: &str, sql: &str) -> String {
    assert!(url.ends_with("/tor_ci"), "never use persistent PostgreSQL");
    let out = Command::new("psql")
        .env("PGOPTIONS", "-c statement_timeout=15000 -c lock_timeout=5000")
        .args(["-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1", "--dbname", url, "--command", sql])
        .output()
        .expect("psql required");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn app(accounts: PgAccounts, catalog: PgCatalog) -> Router {
    let reader = PgReader::new(catalog.pool().clone());
    tor_web::router_with_postgres_reading_auth(
        catalog,
        reader,
        accounts,
        false,
        ORIGIN.to_owned(),
    )
}

async fn get(site: &Router, cookie: &str) -> axum::response::Response {
    site.clone().oneshot(
        Request::builder().uri(PATH).method("GET")
            .header(header::COOKIE, cookie)
            .body(Body::empty()).unwrap(),
    ).await.unwrap()
}

async fn post(site: &Router, cookie: &str, origin: &str, csrf: &str, direction: &str,
    scope: &str, action: &str) -> axum::response::Response {
    let form = format!(
        "csrf_token={csrf}&direction={direction}&scope={scope}&action={action}"
    );
    site.clone().oneshot(
        Request::builder().uri(format!("{PATH}/run")).method("POST")
            .header(header::COOKIE, cookie)
            .header(header::ORIGIN, origin)
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(form)).unwrap()
    ).await.unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18, TOR_G09_OPERATOR_ENABLED=1 and a fixed bucket"]
async fn g09_operator_live_admin_csrf_and_bounded_cycle() {
    assert_eq!(std::env::var("TOR_G09_OPERATOR_ENABLED").as_deref(), Ok("1"));
    assert_eq!(std::env::var("TOR_PAID_S3_BUCKET").as_deref(), Ok(BUCKET));
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let reader_token = generate_session_token();
    let admin_token = generate_session_token();
    let csrf = generate_session_token();
    let reader_id = accounts.register_with_session(
        &format!("g09-reader-{stamp}@example.invalid"),
        "G09 reader",
        &hash_password("G09OperatorReaderPassword2026").unwrap(),
        &session_token_digest(&reader_token),
        NonZeroU32::new(3600).unwrap(),
    ).await.unwrap();
    let admin_id = accounts.register_with_session(
        &format!("g09-admin-{stamp}@example.invalid"),
        "G09 administrator",
        &hash_password("G09OperatorAdministratorPassword2026").unwrap(),
        &session_token_digest(&admin_token),
        NonZeroU32::new(3600).unwrap(),
    ).await.unwrap();
    pg(&url, &format!(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES({admin_id},'administrator',true)"
    ));
    let site = app(accounts, catalog);
    let user_cookie = format!("tor-dev-session={reader_token}; tor-dev-csrf={csrf}");
    let admin_cookie = format!("tor-dev-session={admin_token}; tor-dev-csrf={csrf}");

    assert_eq!(get(&site, "").await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(get(&site, &user_cookie).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        post(&site, &user_cookie, ORIGIN, &csrf, "s3_to_pg", "chapter", "next_cycle")
            .await.status(),
        StatusCode::FORBIDDEN,
    );
    assert_eq!(get(&site, &admin_cookie).await.status(), StatusCode::OK);
    let res = get(&site, &admin_cookie).await;
    assert_eq!(res.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(res.headers()["x-robots-tag"], "noindex, nofollow");
    let html = String::from_utf8(to_bytes(res.into_body(), 256 * 1024)
        .await.unwrap().to_vec()).unwrap();
    assert!(html.contains("Сверка приватного хранилища"));
    assert!(html.contains("name=\"csrf_token\""));
    assert!(html.contains("name=\"action\" value=\"step\""));
    assert!(!html.contains(BUCKET), "private bucket must not be rendered");
    assert!(!html.contains("Удалить объект"));

    // A complete checkpoint can only be reset using same-origin POST + CSRF.
    pg(&url, &format!(
        "INSERT INTO g09_private_inventory_scans
            (bucket,scope,cycle,scanned,registered,completed)
         VALUES('{BUCKET}','chapter',1,3,3,true)"
    ));
    assert_eq!(
        post(&site, &admin_cookie, "https://attacker.invalid", &csrf,
             "s3_to_pg", "chapter", "next_cycle").await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(&site, &admin_cookie, ORIGIN, "wrong",
             "s3_to_pg", "chapter", "next_cycle").await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(&site, &admin_cookie, ORIGIN, &csrf,
             "s3_to_pg", "cover", "next_cycle").await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        post(&site, &admin_cookie, ORIGIN, &csrf,
             "s3_to_pg", "chapter", "resume_exhausted").await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        pg(&url, "SELECT COUNT(*) FROM g09_operator_requests"),
        "0", "invalid actions cannot reach the audit/worker",
    );
    let accepted = post(
        &site, &admin_cookie, ORIGIN, &csrf,
        "s3_to_pg", "chapter", "next_cycle",
    ).await;
    assert_eq!(accepted.status(), StatusCode::SEE_OTHER);
    assert_eq!(accepted.headers()[header::LOCATION], PATH);
    let checkpoint = pg(&url, &format!(
        "SELECT cycle || ':' || scanned || ':' || completed
         FROM g09_private_inventory_scans
         WHERE bucket='{BUCKET}' AND scope='chapter'"
    ));
    assert_eq!(checkpoint, "2:0:f");
    assert_eq!(
        pg(&url, &format!(
            "SELECT direction || ':' || scope || ':' || action
             FROM g09_operator_requests WHERE actor_user_id={admin_id}"
        )),
        "s3_to_pg:chapter:next_cycle"
    );
    assert_eq!(
        post(&site, &admin_cookie, ORIGIN, &csrf,
             "s3_to_pg", "chapter", "next_cycle").await.status(),
        StatusCode::CONFLICT,
        "an unfinished cycle cannot be overwritten",
    );
    assert_eq!(
        get(&site, &admin_cookie).await.status(),
        StatusCode::OK,
    );
    pg(&url, &format!(
        "UPDATE account_roles SET is_active=false
         WHERE user_id={admin_id} AND role='administrator'"
    ));
    assert_eq!(get(&site, &admin_cookie).await.status(), StatusCode::FORBIDDEN);
    assert_eq!(
        post(&site, &admin_cookie, ORIGIN, &csrf,
             "s3_to_pg", "chapter", "step").await.status(),
        StatusCode::FORBIDDEN,
    );
    let still_reader: i64 = reader_id;
    assert!(still_reader > 0);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 and operator feature absent"]
async fn g09_operator_disabled_by_default() {
    assert_ne!(std::env::var("TOR_G09_OPERATOR_ENABLED").as_deref(), Ok("1"));
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool().clone());
    let site = app(accounts, catalog);
    assert_eq!(get(&site, "").await.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        post(&site, "", ORIGIN, "csrf", "s3_to_pg", "chapter", "step")
            .await.status(),
        StatusCode::NOT_FOUND
    );
}
