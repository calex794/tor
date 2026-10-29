//! A-03: real HTTP registration/login does not expose tokens or IP metadata.
//! The accounts crate separately verifies the persisted PostgreSQL event rows.
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::{
    net::SocketAddr,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::session_token_digest,
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn request(
    method: &str,
    path: &str,
    body: &str,
    cookies: &str,
    ip: [u8; 4],
    ua: &str,
) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::ORIGIN, ORIGIN)
        .header(header::COOKIE, cookies)
        .header(header::USER_AGENT, ua)
        // A forged header is never a substitute for the verified direct peer.
        .header("x-forwarded-for", "192.0.2.99")
        .body(Body::from(body.to_owned()))
        .expect("HTTP fixture");
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from((ip, 40001))));
    req
}

async fn send(app: &Router, req: Request<Body>) -> Response {
    app.clone().oneshot(req).await.expect("Axum response")
}

fn cookie(reply: &Response, prefix: &str) -> String {
    reply
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with(prefix))
        .expect("session / CSRF cookie")
        .split(';')
        .next()
        .expect("cookie field")
        .to_owned()
}

async fn csrf(app: &Router, path: &str) -> (String, String) {
    let page = send(
        app,
        request("GET", path, "", "", [198, 51, 100, 3], "Test/1"),
    )
    .await;
    assert_eq!(page.status(), StatusCode::OK);
    let value = cookie(&page, "tor-dev-csrf=");
    let html = to_bytes(page.into_body(), 128_000)
        .await
        .expect("form body");
    let html = String::from_utf8(html.to_vec()).expect("UTF-8 form");
    let token = html
        .split("name=\"csrf_token\" value=\"")
        .nth(1)
        .expect("CSRF hidden input")
        .split('"')
        .next()
        .expect("CSRF token")
        .to_owned();
    (value, token)
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn access_audit_two_http_sessions_survive_failed_login_and_fake_forwarded_ip() {
    let url = std::env::var("TEST_DATABASE_URL").expect("CI PostgreSQL URL");
    assert!(url.ends_with("/tor_ci"), "never use production database");
    let catalog = PgCatalog::connect(&url).await.expect("catalog");
    let reader = PgReader::new(catalog.pool().clone());
    let accounts = PgAccounts::from_pool(catalog.pool().clone());
    accounts.probe().await.expect("schema and tor_app");
    let lookup = accounts.clone();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        reader,
        accounts,
        false,
        ORIGIN.to_owned(),
    );

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("wall clock")
        .as_nanos();
    let email = format!("audit-{nonce}@example.invalid");
    let (csrf_cookie, csrf) = csrf(&app, "/register").await;
    let registration = format!(
        "email=audit-{nonce}%40example.invalid&display_name=AuditReader&password=LongSecurePassword2026&csrf_token={csrf}"
    );
    let created = send(
        &app,
        request(
            "POST",
            "/register",
            &registration,
            &csrf_cookie,
            [198, 51, 100, 12],
            "Chrome/120 Safari/537",
        ),
    )
    .await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let first = cookie(&created, "tor-dev-session=");
    let user_id = lookup
        .find_user(&email)
        .await
        .expect("account lookup")
        .expect("new account")
        .id;
    let first_token = first.strip_prefix("tor-dev-session=").expect("cookie name");
    assert_eq!(
        lookup
            .active_session_user(&session_token_digest(first_token))
            .await
            .expect("session lookup"),
        Some(user_id)
    );

    let (csrf_cookie, csrf) = csrf(&app, "/login").await;
    let invalid = format!(
        "email=audit-{nonce}%40example.invalid&password=wrong&csrf_token={csrf}"
    );
    let rejected = send(
        &app,
        request(
            "POST",
            "/login",
            &invalid,
            &csrf_cookie,
            [203, 0, 113, 41],
            "Firefox/120",
        ),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
    assert!(
        rejected.headers().get_all(header::SET_COOKIE).iter().all(|v| {
            !v.to_str().unwrap_or_default().starts_with("tor-dev-session=")
        }),
        "a failed password must never mint a session"
    );

    let good = format!(
        "email=audit-{nonce}%40example.invalid&password=LongSecurePassword2026&csrf_token={csrf}"
    );
    let logged = send(
        &app,
        request(
            "POST",
            "/login",
            &good,
            &csrf_cookie,
            [203, 0, 113, 41],
            "Firefox/120",
        ),
    )
    .await;
    assert_eq!(logged.status(), StatusCode::SEE_OTHER);
    let second = cookie(&logged, "tor-dev-session=");
    assert_ne!(first, second, "each login creates a distinct session");
    let second_token = second.strip_prefix("tor-dev-session=").expect("cookie name");
    assert_eq!(
        lookup
            .active_session_user(&session_token_digest(second_token))
            .await
            .expect("session lookup"),
        Some(user_id)
    );

    for session in [first, second] {
        let page = send(
            &app,
            request("GET", "/me", "", &session, [192, 0, 2, 13], "Test/1"),
        )
        .await;
        assert_eq!(page.status(), StatusCode::OK);
        let html = to_bytes(page.into_body(), 128_000)
            .await
            .expect("private profile");
        assert!(!html.windows(11).any(|window| window == b"198.51.100."),
            "private profile must not display audit IP");
    }
}
