//! A-07: authenticated password rotation on a disposable PostgreSQL instance.
use axum::{
    body::{to_bytes, Body},
    extract::ConnectInfo,
    http::{header, HeaderValue, Request, StatusCode},
    response::Response,
    Router,
};
use std::{
    net::SocketAddr,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";
const OLD_PASSWORD: &str = "OriginalSecurePassword2026";
const NEW_PASSWORD: &str = "ReplacementSecurePassword2026";

fn request(method: &str, path: &str, data: &str, cookies: &str, origin: &str) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, cookies)
        .header(header::ORIGIN, origin)
        .body(Body::from(data.to_owned()))
        .expect("request");
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 41007))));
    request
}

fn from_peer(mut req: Request<Body>, ip: [u8; 4]) -> Request<Body> {
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from((ip, 41007))));
    req
}

async fn send(app: &Router, req: Request<Body>) -> Response {
    app.clone().oneshot(req).await.expect("HTTP response")
}

fn cookie(reply: &Response, name: &str) -> String {
    reply
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .find_map(|header| {
            let value = header.to_str().ok()?;
            value
                .starts_with(name)
                .then(|| value.split(';').next().unwrap().to_owned())
        })
        .expect("session or CSRF Set-Cookie")
}

async fn html(reply: Response) -> String {
    let bytes = to_bytes(reply.into_body(), 128 * 1024)
        .await
        .expect("bounded HTML");
    String::from_utf8(bytes.to_vec()).expect("UTF-8 HTML")
}

fn csrf(html: &str) -> &str {
    html.split("name=\"csrf_token\" value=\"")
        .nth(1)
        .expect("CSRF field")
        .split('"')
        .next()
        .expect("CSRF token")
}

async fn login(app: &Router, email: &str, password: &str) -> Response {
    let page = send(app, request("GET", "/login", "", "", ORIGIN)).await;
    assert_eq!(page.status(), StatusCode::OK);
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let markup = html(page).await;
    let data = format!(
        "email={email}&password={password}&csrf_token={}",
        csrf(&markup)
    );
    send(app, request("POST", "/login", &data, &csrf_cookie, ORIGIN)).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Requires isolated PostgreSQL 18 TEST_DATABASE_URL ending /tor_ci"]
async fn a_07_password_rotation_revokes_old_sessions_and_password_pg() {
    tokio::time::timeout(
        std::time::Duration::from_secs(120),
        password_rotation_impl(),
    )
    .await
    .expect("A-07 PostgreSQL rotation exceeded 120 seconds");
}

async fn password_rotation_impl() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "Never use production database");
    let catalog = PgCatalog::connect(&url).await.expect("PostgreSQL schema");
    let pool = catalog.pool().clone();
    let reader = PgReader::new(catalog.pool().clone());
    let accounts = PgAccounts::from_pool(catalog.pool().clone());
    accounts.probe().await.expect("accounts schema");
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        reader,
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let email = format!("a07{unique}%40example.invalid");

    let register_page = send(&app, request("GET", "/register", "", "", ORIGIN)).await;
    assert_eq!(register_page.status(), StatusCode::OK);
    let register_csrf_cookie = cookie(&register_page, "tor-dev-csrf=");
    let register_html = html(register_page).await;
    let registration = format!(
        "email={email}&display_name=AccountSecurity&password={OLD_PASSWORD}&csrf_token={}",
        csrf(&register_html)
    );
    let created = send(
        &app,
        from_peer(request(
            "POST",
            "/register",
            &registration,
            &register_csrf_cookie,
            ORIGIN,
        ), [198, 51, 100, 21]),
    )
    .await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let first_session = cookie(&created, "tor-dev-session=");

    let second_login = login(&app, &email, OLD_PASSWORD).await;
    assert_eq!(second_login.status(), StatusCode::SEE_OTHER);
    let second_session = cookie(&second_login, "tor-dev-session=");
    assert_ne!(first_session, second_session);

    let no_auth = send(
        &app,
        request("GET", "/me/settings/password", "", "", ORIGIN),
    )
    .await;
    assert_eq!(no_auth.status(), StatusCode::SEE_OTHER);
    let settings = send(
        &app,
        request("GET", "/me/settings/password", "", &first_session, ORIGIN),
    )
    .await;
    assert_eq!(settings.status(), StatusCode::OK);
    assert_eq!(settings.headers()[header::CACHE_CONTROL], "no-store");
    let settings_csrf_cookie = cookie(&settings, "tor-dev-csrf=");
    let settings_html = html(settings).await;
    assert!(settings_html.contains("Смена пароля"));
    assert!(!settings_html.contains(OLD_PASSWORD));
    let csrf = csrf(&settings_html);
    let change = format!(
        "current_password={OLD_PASSWORD}&new_password={NEW_PASSWORD}&confirm_password={NEW_PASSWORD}&csrf_token={csrf}"
    );
    let cookies = format!("{first_session}; {settings_csrf_cookie}");

    let missing_csrf = send(
        &app,
        request(
            "POST",
            "/me/settings/password",
            &change,
            &first_session,
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(missing_csrf.status(), StatusCode::FORBIDDEN);

    let foreign_origin = send(
        &app,
        request(
            "POST",
            "/me/settings/password",
            &change,
            &cookies,
            "https://evil.invalid",
        ),
    )
    .await;
    assert_eq!(foreign_origin.status(), StatusCode::FORBIDDEN);

    let bad_old = change.replace(OLD_PASSWORD, "WrongOldPassword2026");
    let wrong = send(
        &app,
        request("POST", "/me/settings/password", &bad_old, &cookies, ORIGIN),
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        send(&app, request("GET", "/me", "", &second_session, ORIGIN))
            .await
            .status(),
        StatusCode::OK
    );

    let mut change_request = from_peer(
        request("POST", "/me/settings/password", &change, &cookies, ORIGIN),
        [203, 0, 113, 42],
    );
    change_request.headers_mut().insert(
        "x-forwarded-for",
        HeaderValue::from_static("192.0.2.222"),
    );
    change_request.headers_mut().insert(
        "x-real-ip",
        HeaderValue::from_static("192.0.2.223"),
    );
    let rotated = send(&app, change_request).await;
    assert_eq!(rotated.status(), StatusCode::SEE_OTHER);
    assert_eq!(rotated.headers()[header::LOCATION], "/me");
    let fresh_session = cookie(&rotated, "tor-dev-session=");
    assert_ne!(fresh_session, first_session);
    assert_ne!(fresh_session, second_session);
    let digest = session_token_digest(fresh_session.split_once('=').unwrap().1);
    let audit: (Option<String>, String) = sqlx::query_as(
        "SELECT host(e.verified_ip), e.event_kind FROM account_login_events e
         JOIN sessions s ON s.session_id=e.session_id WHERE s.token_hash=$1",
    )
    .bind(digest.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audit.0.as_deref(), Some("203.0.113.42"));
    assert_eq!(audit.1, "password_change");

    for old in [&first_session, &second_session] {
        let declined = send(&app, request("GET", "/me", "", old, ORIGIN)).await;
        assert_eq!(declined.status(), StatusCode::SEE_OTHER);
        assert_eq!(declined.headers()[header::LOCATION], "/login");
    }
    assert_eq!(
        send(&app, request("GET", "/me", "", &fresh_session, ORIGIN))
            .await
            .status(),
        StatusCode::OK
    );

    let replay = send(
        &app,
        request("POST", "/me/settings/password", &change, &cookies, ORIGIN),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::SEE_OTHER);
    assert_eq!(replay.headers()[header::LOCATION], "/login");

    let rejected = login(&app, &email, OLD_PASSWORD).await;
    assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
    let admitted = login(&app, &email, NEW_PASSWORD).await;
    assert_eq!(admitted.status(), StatusCode::SEE_OTHER);
}
