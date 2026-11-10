//! S039/MONEY-04: operator-only HTTP payout posting in disposable tor_ci.
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
const ADMIN_PASSWORD: &str = "S039PayoutPostingAdministratorPassword2026";

fn pg(url: &str, sql: &str) -> String {
    assert!(url.ends_with("/tor_ci"), "never touch retained PostgreSQL");
    let out = Command::new("psql")
        .env(
            "PGOPTIONS",
            "-c statement_timeout=15000 -c lock_timeout=5000",
        )
        .args([
            "-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1", "--dbname", url,
            "--command", sql,
        ])
        .output()
        .expect("psql required for PostgreSQL acceptance");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

fn app(accounts: PgAccounts, catalog: PgCatalog) -> Router {
    let reader = PgReader::new(catalog.pool().clone());
    tor_web::router_with_postgres_reading_auth(catalog, reader, accounts, false, ORIGIN.to_owned())
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

async fn html(res: axum::response::Response) -> String {
    let bytes = to_bytes(res.into_body(), 128 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
#[ignore = "requires disposable tor_ci and disabled payout-post flag"]
async fn s039_payout_post_http_closed_by_default() {
    assert_ne!(
        std::env::var("TOR_S039_PAYOUT_POST_HTTP_ENABLED").as_deref(),
        Ok("1")
    );
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool().clone());
    let site = app(accounts, catalog);
    let result = request(
        &site,
        "POST",
        "/admin/finance/payouts/1/post",
        "",
        ORIGIN,
        "invalid=true",
    )
    .await;
    assert_eq!(result.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci and explicit payout-post opt-in"]
async fn s039_payout_post_http_role_password_csrf_exact_once_and_replay() {
    assert_eq!(
        std::env::var("TOR_S039_PAYOUT_POST_HTTP_ENABLED").as_deref(),
        Ok("1")
    );
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let owner_token = generate_session_token();
    let admin_token = generate_session_token();
    let csrf = generate_session_token();
    let owner = accounts
        .register_with_session(
            &format!("s039-post-http-owner-{stamp}@example.invalid"),
            "S039 author",
            &hash_password("S039PayoutPostingOwnerPassword2026").unwrap(),
            &session_token_digest(&owner_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let admin = accounts
        .register_with_session(
            &format!("s039-post-http-admin-{stamp}@example.invalid"),
            "S039 operator",
            &hash_password(ADMIN_PASSWORD).unwrap(),
            &session_token_digest(&admin_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
             VALUES('ci:s039:post:http:{stamp}','ru','S039 posting','public',{owner})"
        ),
    );
    pg(
        &url,
        &format!(
            "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
             VALUES({owner},1000,'RUB')"
        ),
    );
    pg(
        &url,
        &format!(
            "INSERT INTO account_roles(user_id,role,is_active)
             VALUES({admin},'administrator',true)"
        ),
    );
    let finance = PgCreatorFinance::new(pool.clone());
    let id = finance
        .reserve_once(
            owner,
            600,
            &IdempotencyKey::parse(&format!("{}8", "W".repeat(42))).unwrap(),
        )
        .await
        .unwrap()
        .id;
    finance
        .reserve_once(
            owner,
            300,
            &IdempotencyKey::parse(&format!("{}8", "Z".repeat(42))).unwrap(),
        )
        .await
        .unwrap();
    let site = app(accounts, catalog);
    let path = format!("/admin/finance/payouts/{id}/post");
    let owner_cookie = format!("tor-dev-session={owner_token}; tor-dev-csrf={csrf}");
    let admin_cookie = format!("tor-dev-session={admin_token}; tor-dev-csrf={csrf}");
    let form = format!(
        "csrf_token={csrf}&reason=Approve+internally&current_password={ADMIN_PASSWORD}"
    );

    assert_eq!(
        request(&site, "POST", &path, "", ORIGIN, &form)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        request(&site, "POST", &path, &owner_cookie, ORIGIN, &form)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let queue = request(
        &site,
        "GET",
        "/admin/finance/payouts",
        &admin_cookie,
        ORIGIN,
        "",
    )
    .await;
    assert_eq!(queue.status(), StatusCode::OK);
    let markup = html(queue).await;
    assert!(markup.contains(&format!("action=\"{path}\"")));
    assert!(markup.contains("name=\"current_password\""));
    assert!(!markup.contains("name=\"amount_minor\""));

    assert_eq!(
        request(
            &site, "POST", &path, &admin_cookie, "https://evil.invalid", &form
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &site,
            "POST",
            &path,
            &admin_cookie,
            ORIGIN,
            &format!(
                "csrf_token=invalid&reason=Approve+internally&current_password={ADMIN_PASSWORD}"
            ),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        request(
            &site,
            "POST",
            &path,
            &admin_cookie,
            ORIGIN,
            &format!("csrf_token={csrf}&reason=Approve+internally&current_password=incorrect"),
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT state FROM finance_creator_payout_requests WHERE id={id}")
        ),
        "pending"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={owner}")
        ),
        "1000"
    );

    let posted = request(&site, "POST", &path, &admin_cookie, ORIGIN, &form).await;
    assert_eq!(posted.status(), StatusCode::OK);
    assert_eq!(posted.headers()[header::CACHE_CONTROL], "no-store");
    let body = html(posted).await;
    assert!(body.contains("Банковский перевод не выполнялся"));
    assert!(!body.contains(ADMIN_PASSWORD));
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
            &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={owner}")
        ),
        "400"
    );
    assert_eq!(finance.totals(owner).await.unwrap().reserved_minor, 300);

    let replay = request(&site, "POST", &path, &admin_cookie, ORIGIN, &form).await;
    assert_eq!(replay.status(), StatusCode::OK);
    assert!(html(replay).await.contains("списание не повторялось"));
    assert_eq!(
        request(
            &site,
            "POST",
            &path,
            &admin_cookie,
            ORIGIN,
            &format!(
                "csrf_token={csrf}&reason=Changed&current_password={ADMIN_PASSWORD}"
            )
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let (postings, debits): (i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT count(*) FROM finance_creator_payout_postings WHERE payout_request_id=$1),
             (SELECT count(*) FROM reader_wallet_entries
              WHERE user_id=$2 AND kind='creator_payout_debit')",
    )
    .bind(id)
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((postings, debits), (1, 1));
    assert_eq!(
        pg(
            &url,
            &format!("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id={owner}")
        ),
        "400"
    );

    pg(
        &url,
        &format!(
            "UPDATE account_roles SET is_active=false
             WHERE user_id={admin} AND role='administrator'"
        ),
    );
    assert_eq!(
        request(&site, "POST", &path, &admin_cookie, ORIGIN, &form)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    pool.close().await;
}
