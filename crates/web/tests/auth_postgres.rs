//! A-02/A-03: protected forms and revocable sessions on disposable PostgreSQL.
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::net::SocketAddr;
use tor_accounts::repositories::PgAccounts;
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn request(method: &str, uri: &str, body: &str, cookies: &str, origin: &str) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, cookies)
        .header(header::ORIGIN, origin)
        .body(Body::from(body.to_owned()))
        .expect("HTTP");
    req.extensions_mut().insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 40001))));
    req
}
async fn send(app: &Router, req: Request<Body>) -> Response {
    app.clone().oneshot(req).await.expect("router response")
}
async fn body_of(reply: Response) -> String {
    let body = to_bytes(reply.into_body(), 128 * 1024).await.expect("body");
    String::from_utf8(body.to_vec()).expect("HTML UTF-8")
}
fn csrf_token(html: &str) -> &str {
    html.split("name=\"csrf_token\" value=\"")
        .nth(1).expect("csrf form field")
        .split('"').next().expect("csrf value")
}
fn cookie(reply: &Response, prefix: &str) -> String {
    reply.headers().get_all(header::SET_COOKIE).iter()
        .find_map(|v| {
            let s = v.to_str().ok()?;
            s.starts_with(prefix).then(|| s.split(';').next()?.to_owned())
        }).expect("Set-Cookie")
}
#[tokio::test]
#[ignore = "Requires TEST_DATABASE_URL on isolated PostgreSQL tor_ci"]
async fn registration_login_logout_origin_csrf_and_private_profile() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database URL");
    let catalog = PgCatalog::connect(&url).await.expect("role and schema");
    let pool = catalog.pool();
    let reader = PgReader::new(pool.clone());
    reader.probe().await.expect("reading schema");
    let accounts = PgAccounts::from_pool(pool.clone());
    accounts.probe().await.expect("accounts schema");
    assert!(url.ends_with("/tor_ci"), "Never run auth integration against production");
    let lookup = accounts.clone();

    let app = tor_web::router_with_postgres_reading_auth(
        catalog, reader, accounts, false, ORIGIN.to_owned()
    );
    let page = send(&app, request("GET", "/register", "", "", ORIGIN)).await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()[header::CACHE_CONTROL], "no-store");
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let page_body = body_of(page).await;
    let csrf = csrf_token(&page_body);
    assert!(page_body.contains("Создать учётную запись"));

    let form = "email=tor_ci_account%40example.com&display_name=CIReader&password=LongSecurePassword2026&csrf_token=".to_owned() + csrf;
    assert_eq!(
        send(&app, request("POST", "/register", &form, "", ORIGIN)).await.status(),
        StatusCode::FORBIDDEN, "Registration requires browser CSRF token cookie"
    );
    assert_eq!(
        send(&app, request("POST", "/register", &form, &csrf_cookie, "https://evil.example")).await.status(),
        StatusCode::FORBIDDEN, "Origin cannot be overridden by the CSRF token"
    );
    let created = send(&app, request("POST", "/register", &form, &csrf_cookie, ORIGIN)).await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    assert_eq!(created.headers()[header::LOCATION], "/me");
    let session = cookie(&created, "tor-dev-session=");
    assert!(session.contains("tor-dev-session="));
    assert!(created.headers().get_all(header::SET_COOKIE).iter().any(|v| v.to_str().unwrap().contains("HttpOnly")));

    let hash = lookup.find_user("tor_ci_account@example.com")
        .await.expect("account lookup").expect("persisted account").password_hash;
    assert!(hash.starts_with("$argon2id$"));
    assert!(!hash.contains("LongSecurePassword2026"));

    let private = send(&app, request("GET", "/me", "", &session, ORIGIN)).await;
    assert_eq!(private.status(), StatusCode::OK);
    assert_eq!(private.headers()[header::CACHE_CONTROL], "no-store");
    let new_csrf_cookie = cookie(&private, "tor-dev-csrf=");
    let private_html = body_of(private).await;
    assert!(private_html.contains("CIReader"));
    assert!(!private_html.contains("LongSecurePassword2026"));
    assert!(private_html.contains("href=\"/me\""));

    assert_eq!(
        send(&app, request("POST", "/register", &form, &csrf_cookie, ORIGIN)).await.status(),
        StatusCode::CONFLICT, "Email uniqueness enforced by PostgreSQL"
    );
    let login_page = send(&app, request("GET", "/login", "", "", ORIGIN)).await;
    let login_csrf_cookie = cookie(&login_page, "tor-dev-csrf=");
    let login_html = body_of(login_page).await;
    let login_csrf = csrf_token(&login_html);
    let bad_login = format!("email=tor_ci_account%40example.com&password=badpass&csrf_token={login_csrf}");
    let bad = send(&app, request("POST", "/login", &bad_login, &login_csrf_cookie, ORIGIN)).await;
    assert_eq!(bad.status(), StatusCode::UNAUTHORIZED);
    let good_login = format!("email=tor_ci_account%40example.com&password=LongSecurePassword2026&csrf_token={login_csrf}");
    let logged = send(&app, request("POST", "/login", &good_login, &login_csrf_cookie, ORIGIN)).await;
    assert_eq!(logged.status(), StatusCode::SEE_OTHER);

    let logout_csrf = csrf_token(&private_html);
    let combined_cookies = format!("{session}; {new_csrf_cookie}");
    let logout = format!("csrf_token={logout_csrf}");
    assert_eq!(
        send(&app, request("POST", "/logout", &logout, &combined_cookies, "https://evil.example")).await.status(),
        StatusCode::FORBIDDEN
    );
    let left = send(&app, request("POST", "/logout", &logout, &combined_cookies, ORIGIN)).await;
    assert_eq!(left.status(), StatusCode::SEE_OTHER);
    assert!(left.headers().get_all(header::SET_COOKIE).iter().any(|v| v.to_str().unwrap().contains("Max-Age=0")));
    let revoked = send(&app, request("GET", "/me", "", &session, ORIGIN)).await;
    assert_eq!(revoked.status(), StatusCode::SEE_OTHER);
    assert_eq!(revoked.headers()[header::LOCATION], "/login");
}
