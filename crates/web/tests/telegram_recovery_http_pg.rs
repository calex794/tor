//! A-07 / S004: isolated HTTP-to-PostgreSQL pairing and unlink smoke test.
//! The bot update is represented by the authenticated repository claim;
//! this test never calls the external Telegram API.
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{header, Request, StatusCode},
    Router,
};
use std::{
    net::SocketAddr,
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
    let mut req = req;
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 41906))));
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

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn a_07_browser_receives_reset_only_after_linked_telegram_approval() {
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
    let email = format!("tg-browser-{nonce}@example.invalid");
    let session = generate_session_token();
    let old_password = "SecureInitialTelegramRecovery2026";
    let user = accounts
        .register_with_session(
            &email,
            "Reader",
            &hash_password(old_password).unwrap(),
            &session_token_digest(&session),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let tg_id = 13_000_000_i64 + user;
    let binding_nonce = session_token_digest(&format!("browser-binding-{user}"));
    assert!(accounts
        .begin_telegram_link(
            user,
            &session_token_digest(&session),
            &accounts
                .password_hash_for_user(user)
                .await
                .unwrap()
                .unwrap(),
            &binding_nonce,
        )
        .await
        .unwrap());
    assert!(accounts
        .claim_telegram_link(&binding_nonce, tg_id, tg_id)
        .await
        .unwrap());
    let codes: [[u8; 32]; 8] =
        std::array::from_fn(|i| session_token_digest(&format!("http-recovery-code-{user}-{i}")));
    assert_eq!(
        accounts
            .finish_telegram_link(user, &session_token_digest(&session), &codes,)
            .await
            .unwrap(),
        Some(tg_id)
    );
    let form = request(&app, "GET", "/recover/telegram", "", String::new()).await;
    assert_eq!(form.status(), StatusCode::OK);
    let csrf_cookie = form
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
    let csrf = value_after(&html(form).await, "name=\"csrf_token\" value=\"").to_owned();
    let started = request(
        &app,
        "POST",
        "/recover/telegram",
        &csrf_cookie,
        format!("email={email}&csrf_token={csrf}"),
    )
    .await;
    assert_eq!(started.status(), StatusCode::OK);
    let browser_cookie = started
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap())
        .find(|v| v.starts_with("tor-dev-tg-recover="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let waiting_html = html(started).await;
    let match_code = &waiting_html[waiting_html.find("Код сверки: <strong>").unwrap() + 20
        ..waiting_html.find("Код сверки: <strong>").unwrap() + 26];
    assert_eq!(match_code.len(), 6);
    assert!(!waiting_html.contains("/reset-password#"));
    let cookies = format!("{csrf_cookie}; {browser_cookie}");
    let before = request(
        &app,
        "POST",
        "/recover/telegram/complete",
        &cookies,
        format!("csrf_token={csrf}"),
    )
    .await;
    assert_eq!(before.status(), StatusCode::CONFLICT);
    let dispatch = accounts
        .pending_telegram_recovery_notifications()
        .await
        .unwrap();
    let item = dispatch
        .iter()
        .find(|item| item.private_chat_id == tg_id)
        .unwrap();
    assert_eq!(item.match_code, match_code);
    assert!(!accounts
        .decide_telegram_recovery(item.id, tg_id + 1, tg_id, true)
        .await
        .unwrap());
    assert!(accounts
        .decide_telegram_recovery(item.id, tg_id, tg_id, true)
        .await
        .unwrap());
    let foreign = request(
        &app,
        "POST",
        "/recover/telegram/complete",
        &format!(
            "{csrf_cookie}; tor-dev-tg-recover={}",
            generate_session_token()
        ),
        format!("csrf_token={csrf}"),
    )
    .await;
    assert_eq!(foreign.status(), StatusCode::CONFLICT);
    let ready = request(&app, "GET", "/recover/telegram", &cookies, String::new()).await;
    let ready_html = html(ready).await;
    assert!(ready_html.contains("Продолжить смену пароля"));
    let complete = request(
        &app,
        "POST",
        "/recover/telegram/complete",
        &cookies,
        format!("csrf_token={csrf}"),
    )
    .await;
    assert_eq!(complete.status(), StatusCode::OK);
    let content = html(complete).await;
    let reset_token = value_after(&content, "/reset-password#").to_owned();
    assert_eq!(reset_token.len(), 43);
    let replay = request(
        &app,
        "POST",
        "/recover/telegram/complete",
        &cookies,
        format!("csrf_token={csrf}"),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::CONFLICT);
    let new_password = "SecureNewTelegramPassword2026";
    let rotated = request(&app,"POST","/reset-password",&csrf_cookie,
        format!("csrf_token={csrf}&token={reset_token}&new_password={new_password}&confirm_password={new_password}"))
        .await;
    assert_eq!(rotated.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        accounts
            .active_session_user(&session_token_digest(&session))
            .await
            .unwrap(),
        None
    );
}
