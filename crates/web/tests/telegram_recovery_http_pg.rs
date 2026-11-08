//! A-07 / S004: isolated HTTP-to-PostgreSQL pairing and unlink smoke test.
//! The bot update is represented by the authenticated repository claim;
//! this test never calls the external Telegram API.
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use std::{
    num::NonZeroU32,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    cookies: &str,
    body: String,
) -> axum::response::Response {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, cookies)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body))
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

async fn html(response: axum::response::Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 256 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

fn value_after<'a>(body: &'a str, prefix: &str) -> &'a str {
    let start = body.find(prefix).expect("expected private page field") + prefix.len();
    &body[start..start + 43]
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn a_07_browser_link_unlink_origin_csrf_password_and_backup_invalidation() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1"));
    std::env::set_var("TOR_TELEGRAM_RECOVERY_ENABLED", "1");
    std::env::set_var("TOR_TELEGRAM_BOT_USERNAME", "TorRecoveryBot");
    let catalog = tor_books::repositories::PgCatalog::connect(&url)
        .await
        .unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        tor_reading::repositories::PgReader::new(pool),
        accounts.clone(),
        false,
        ORIGIN.into(),
    );
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let session = generate_session_token();
    let password = "SecureOriginalPasswordTelegram2026";
    let user = accounts
        .register_with_session(
            &format!("tg-http-{nonce}@example.invalid"),
            "Reader",
            &hash_password(password).unwrap(),
            &session_token_digest(&session),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        request(&app, "GET", "/me/security/telegram", "", String::new())
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let page = request(
        &app,
        "GET",
        "/me/security/telegram",
        &format!("tor-dev-session={session}"),
        String::new(),
    )
    .await;
    assert_eq!(page.status(), StatusCode::OK);
    let csrf_cookie = page
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap())
        .find(|v| v.starts_with("tor-dev-csrf="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let page = html(page).await;
    let csrf = value_after(&page, "name=\"csrf_token\" value=\"").to_owned();
    let cookies = format!("tor-dev-session={session}; {csrf_cookie}");
    let wrong = request(
        &app,
        "POST",
        "/me/security/telegram/begin",
        &cookies,
        format!("csrf_token={csrf}&current_password=wrong"),
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::FORBIDDEN);
    let good = request(
        &app,
        "POST",
        "/me/security/telegram/begin",
        &cookies,
        format!("csrf_token={csrf}&current_password={password}"),
    )
    .await;
    assert_eq!(good.status(), StatusCode::OK);
    let body = html(good).await;
    let link_nonce = value_after(&body, "?start=bind_");
    let telegram_id = 12_000_000_i64 + user;
    assert!(accounts
        .claim_telegram_link(&session_token_digest(link_nonce), telegram_id, telegram_id)
        .await
        .unwrap());
    let confirm = request(
        &app,
        "POST",
        "/me/security/telegram/confirm",
        &cookies,
        format!("csrf_token={csrf}"),
    )
    .await;
    assert_eq!(confirm.status(), StatusCode::OK);
    let shown = html(confirm).await;
    let backup = value_after(&shown, "<li><code>").to_owned();
    assert_eq!(backup.len(), 43);
    let page = html(
        request(
            &app,
            "GET",
            "/me/security/telegram",
            &cookies,
            String::new(),
        )
        .await,
    )
    .await;
    assert!(page.contains("Отвязать Telegram"));
    let denied = request(
        &app,
        "POST",
        "/me/security/telegram/unlink",
        &cookies,
        format!("csrf_token={csrf}&current_password=wrong"),
    )
    .await;
    assert_eq!(denied.status(), StatusCode::FORBIDDEN);
    let invalid_csrf = request(
        &app,
        "POST",
        "/me/security/telegram/unlink",
        &cookies,
        format!("csrf_token=forged&current_password={password}"),
    )
    .await;
    assert_eq!(invalid_csrf.status(), StatusCode::FORBIDDEN);
    let unlinked = request(
        &app,
        "POST",
        "/me/security/telegram/unlink",
        &cookies,
        format!("csrf_token={csrf}&current_password={password}"),
    )
    .await;
    assert_eq!(unlinked.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        accounts
            .telegram_link_state(user, &session_token_digest(&session))
            .await
            .unwrap()
            .linked_user_id,
        None
    );
    let email = format!("tg-http-{nonce}@example.invalid");
    assert!(!accounts
        .consume_backup_code(
            &email,
            &session_token_digest(&format!("tor:backup:v1:{backup}")),
            &session_token_digest(&generate_session_token())
        )
        .await
        .unwrap());
}
