//! S039: default-off internal payout posting HTTP path with disposable tor_ci.
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
    domain::money::IdempotencyKey, repositories::creator_finance::PgCreatorFinance,
};
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";
const PATH: &str = "/admin/finance/payouts";

fn pg(url: &str, sql: &str) -> String {
    assert!(url.ends_with("/tor_ci"), "never use retained DB");
    let output = Command::new("psql")
        .env(
            "PGOPTIONS",
            "-c statement_timeout=15000 -c lock_timeout=5000",
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
            sql,
        ])
        .output()
        .expect("psql required for finance HTTP acceptance");
    assert!(
        output.status.success(),
        "fixture SQL failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn request(
    site: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    origin: &str,
    form: &str,
) -> axum::response::Response {
    let req = Request::builder()
        .uri(path)
        .method(method)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form.to_owned()))
        .unwrap();
    site.clone().oneshot(req).await.unwrap()
}

async fn html(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), 128 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn app(accounts: PgAccounts, catalog: PgCatalog) -> Router {
    let reader = PgReader::new(catalog.pool().clone());
    tor_web::router_with_postgres_reading_auth(catalog, reader, accounts, false, ORIGIN.to_owned())
}

#[tokio::test]
#[ignore = "requires disposable tor_ci with S039 posting flag disabled"]
async fn s039_post_http_disabled_by_default() {
    assert_ne!(
        std::env::var("TOR_S039_PAYOUT_POST_HTTP_ENABLED").as_deref(),
        Ok("1")
    );
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool().clone());
    let site = app(accounts, catalog);
    assert_eq!(
        request(
            &site,
            "POST",
            "/admin/finance/payouts/1/post",
            "",
            ORIGIN,
            "invalid=form"
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci and explicit S039 posting flag"]
async fn s039_post_http_password_origin_csrf_and_exactly_once() {
    assert_eq!(
        std::env::var("TOR_S039_PAYOUT_POST_HTTP_ENABLED").as_deref(),
        Ok("1")
    );
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"), "never use retained PostgreSQL");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let author_password = "S039PayoutAuthorHttp2026";
    let admin_password = "S039PayoutAdminHttp2026";
    let author_token = generate_session_token();
    let admin_token = generate_session_token();
    let csrf = generate_session_token();
    let author = accounts
        .register_with_session(
            &format!("s039-post-http-author-{stamp}@example.invalid"),
            "S039 post author",
            &hash_password(author_password).unwrap(),
            &session_token_digest(&author_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let admin = accounts
        .register_with_session(
            &format!("s039-post-http-admin-{stamp}@example.invalid"),
            "S039 post admin",
            &hash_password(admin_password).unwrap(),
            &session_token_digest(&admin_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
             VALUES('ci:s039:post:http:{stamp}','ru','S039 internal payout','public',{author})"
        ),
    );
    pg(
        &url,
        &format!(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
             VALUES({author},1000,'RUB')"
        ),
    );
    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)
             VALUES({admin},'administrator',true)"
        ),
    );
    let id = PgCreatorFinance::new(pool.clone())
        .reserve_once(
            author,
            600,
            &IdempotencyKey::parse(&format!("{}8", "I".repeat(42))).unwrap(),
        )
        .await
        .unwrap()
        .id;
    let site = app(accounts, catalog);
    let author_cookie = format!("tor-dev-session={author_token}; tor-dev-csrf={csrf}");
    let admin_cookie = format!("tor-dev-session={admin_token}; tor-dev-csrf={csrf}");
    let page = request(&site, "GET", PATH, &admin_cookie, ORIGIN, "").await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(page.headers()["x-robots-tag"], "noindex, nofollow");
    let markup = html(page).await;
    let post_path = format!("{PATH}/{id}/post");
    assert!(markup.contains(&format!("action=\"{post_path}\"")));
    assert!(markup.contains("name=\"expected_amount_minor\" value=\"600\""));
    assert!(markup.contains("НЕ перечисляются"));
    assert_eq!(
        request(&site, "GET", PATH, &author_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let valid = format!(
        "csrf_token={csrf}&expected_amount_minor=600&reason=Internal+posting&current_password={admin_password}"
    );
    assert_eq!(
        request(
            &site,
            "POST",
            &post_path,
            &admin_cookie,
            "https://evil.invalid",
            &valid
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &site, "POST", &post_path, &admin_cookie, ORIGIN,
            &format!("csrf_token=bad&expected_amount_minor=600&reason=Internal+posting&current_password={admin_password}")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &site, "POST", &post_path, &admin_cookie, ORIGIN,
            &format!("csrf_token={csrf}&expected_amount_minor=600&reason=Internal+posting&current_password=wrong")
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(
            &site, "POST", &post_path, &author_cookie, ORIGIN,
            &format!("csrf_token={csrf}&expected_amount_minor=600&reason=Internal+posting&current_password={author_password}")
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT state FROM finance_creator_payout_requests WHERE id={id}")
        ),
        "pending"
    );
    assert_eq!(
        request(
            &site, "POST", &post_path, &admin_cookie, ORIGIN,
            &format!("csrf_token={csrf}&expected_amount_minor=601&reason=Internal+posting&current_password={admin_password}")
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let response = request(&site, "POST", &post_path, &admin_cookie, ORIGIN, &valid).await;
    assert_eq!(response.status(), StatusCode::OK);
    let result_html = html(response).await;
    assert!(result_html.contains("Банковский перевод НЕ выполнен"));
    assert_eq!(
        pg(
            &url,
            &format!("SELECT state FROM finance_creator_payout_requests WHERE id={id}")
        ),
        "posted"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={author}")
        ),
        "400"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM finance_creator_payout_postings WHERE payout_request_id={id}"
            )
        ),
        "1"
    );
    assert_eq!(
        request(&site, "POST", &post_path, &admin_cookie, ORIGIN, &valid)
            .await
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM reader_wallet_entries
                 WHERE user_id={author} AND kind='creator_payout_debit'"
            )
        ),
        "1"
    );
    assert_eq!(
        request(
            &site, "POST", &post_path, &admin_cookie, ORIGIN,
            &format!("csrf_token={csrf}&expected_amount_minor=600&reason=Other&current_password={admin_password}")
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    pg(
        &url,
        &format!(
            "UPDATE account_roles SET is_active=false
             WHERE user_id={admin} AND role='administrator'"
        ),
    );
    assert_eq!(
        request(&site, "POST", &post_path, &admin_cookie, ORIGIN, &valid)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={author}")
        ),
        "400"
    );
    pg(
        &url,
        &format!(
            "UPDATE sessions SET revoked_at=COALESCE(revoked_at,clock_timestamp())
             WHERE user_id={admin}"
        ),
    );
    pool.close().await;
}
