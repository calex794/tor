//! A-03: real PostgreSQL/HTTP session listing and owner-only revocation.
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

fn request(method: &str, path: &str, body: &str, cookies: &str, origin: &str) -> Request<Body> {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, cookies)
        .header(header::ORIGIN, origin)
        .body(Body::from(body.to_owned()))
        .expect("HTTP request");
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 49110))));
    req
}

async fn send(app: &Router, req: Request<Body>) -> Response {
    app.clone().oneshot(req).await.expect("HTTP response")
}

async fn body_of(response: Response) -> String {
    let bytes = to_bytes(response.into_body(), 128 * 1024)
        .await
        .expect("bounded page");
    String::from_utf8(bytes.to_vec()).expect("UTF-8")
}

fn cookie(response: &Response, prefix: &str) -> String {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .find_map(|v| {
            let value = v.to_str().ok()?;
            value
                .starts_with(prefix)
                .then(|| value.split(';').next().unwrap().to_owned())
        })
        .expect("browser cookie")
}

fn csrf(html: &str) -> &str {
    html.split("name=\"csrf_token\" value=\"")
        .nth(1)
        .expect("CSRF hidden field")
        .split('"')
        .next()
        .expect("CSRF value")
}

async fn register(app: &Router, email: &str, display_name: &str) -> String {
    let page = send(app, request("GET", "/register", "", "", ORIGIN)).await;
    assert_eq!(page.status(), StatusCode::OK);
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let body = body_of(page).await;
    let form = format!(
        "email={email}&display_name={display_name}&password=LongSecurePassword2026&csrf_token={}",
        csrf(&body)
    );
    let created = send(
        app,
        request("POST", "/register", &form, &csrf_cookie, ORIGIN),
    )
    .await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    cookie(&created, "tor-dev-session=")
}

async fn login(app: &Router, email: &str) -> String {
    let page = send(app, request("GET", "/login", "", "", ORIGIN)).await;
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let body = body_of(page).await;
    let form = format!(
        "email={email}&password=LongSecurePassword2026&csrf_token={}",
        csrf(&body)
    );
    let login = send(
        app,
        request("POST", "/login", &form, &csrf_cookie, ORIGIN),
    )
    .await;
    assert_eq!(login.status(), StatusCode::SEE_OTHER);
    cookie(&login, "tor-dev-session=")
}

async fn session_id(
    pool: &sqlx::PgPool,
    cookie: &str,
) -> String {
    let token = cookie.split_once('=').expect("session cookie").1;
    sqlx::query_scalar("SELECT session_id::text FROM sessions WHERE token_hash = $1")
        .bind(session_token_digest(token).as_slice())
        .fetch_one(pool)
        .await
        .expect("session UUID")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Requires isolated TEST_DATABASE_URL PostgreSQL tor_ci"]
async fn a_03_owner_session_list_revocation_and_csrf_http_pg() {
    tokio::time::timeout(
        std::time::Duration::from_secs(120),
        run_case(),
    )
    .await
    .expect("session security HTTP PostgreSQL test exceeded 120 seconds");
}

async fn run_case() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "Never use production database");
    let catalog = PgCatalog::connect(&url).await.expect("catalog");
    let pool = catalog.pool().clone();
    let reader = PgReader::new(pool.clone());
    let accounts = PgAccounts::from_pool(pool.clone());
    accounts.probe().await.expect("account schema");
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        reader,
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let a = format!("sessions-a-{nonce}%40example.invalid");
    let b = format!("sessions-b-{nonce}%40example.invalid");
    let first = register(&app, &a, "OwnerA").await;
    let second = login(&app, &a).await;
    let foreign = register(&app, &b, "OwnerB").await;
    let first_id = session_id(&pool, &first).await;
    let second_id = session_id(&pool, &second).await;
    let foreign_id = session_id(&pool, &foreign).await;

    let anon = send(
        &app,
        request("GET", "/me/security/sessions", "", "", ORIGIN),
    )
    .await;
    assert_eq!(anon.status(), StatusCode::SEE_OTHER);

    let shown = send(
        &app,
        request("GET", "/me/security/sessions", "", &first, ORIGIN),
    )
    .await;
    assert_eq!(shown.status(), StatusCode::OK);
    assert_eq!(shown.headers()[header::CACHE_CONTROL], "no-store");
    let csrf_cookie = cookie(&shown, "tor-dev-csrf=");
    let html = body_of(shown).await;
    assert!(html.contains("Активные сеансы"));
    assert!(html.contains(&first_id));
    assert!(html.contains(&second_id));
    assert!(!html.contains(&foreign_id), "cannot view foreign sessions");
    assert!(!html.contains(&first), "never render raw session cookies");
    let token = csrf(&html);
    let cookies = format!("{first}; {csrf_cookie}");
    let foreign_form = format!("session_id={foreign_id}&csrf_token={token}");

    let forged = send(
        &app,
        request(
            "POST",
            "/me/security/sessions",
            &foreign_form,
            &cookies,
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(forged.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        send(&app, request("GET", "/me", "", &foreign, ORIGIN))
            .await
            .status(),
        StatusCode::OK,
        "another reader's session must survive"
    );

    let revoke_other = format!("session_id={second_id}&csrf_token={token}");
    assert_eq!(
        send(
            &app,
            request(
                "POST",
                "/me/security/sessions",
                &revoke_other,
                &first,
                ORIGIN,
            )
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(
            &app,
            request(
                "POST",
                "/me/security/sessions",
                &revoke_other,
                &cookies,
                "https://evil.invalid",
            )
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let success = send(
        &app,
        request("POST", "/me/security/sessions", &revoke_other, &cookies, ORIGIN),
    )
    .await;
    assert_eq!(success.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        success.headers()[header::LOCATION],
        "/me/security/sessions"
    );
    assert_eq!(
        send(&app, request("GET", "/me", "", &second, ORIGIN))
            .await
            .status(),
        StatusCode::SEE_OTHER,
        "selected session must be invalid immediately"
    );
    assert_eq!(
        send(&app, request("GET", "/me", "", &first, ORIGIN))
            .await
            .status(),
        StatusCode::OK,
        "owner's current session remains valid"
    );

    let revoke_self = format!("session_id={first_id}&csrf_token={token}");
    let own = send(
        &app,
        request("POST", "/me/security/sessions", &revoke_self, &cookies, ORIGIN),
    )
    .await;
    assert_eq!(own.status(), StatusCode::SEE_OTHER);
    assert_eq!(own.headers()[header::LOCATION], "/login");
    assert!(
        own.headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .any(|v| v.to_str().unwrap_or_default().contains("Max-Age=0"))
    );
    assert_eq!(
        send(&app, request("GET", "/me", "", &first, ORIGIN))
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
}
