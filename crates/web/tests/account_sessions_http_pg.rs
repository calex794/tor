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
    services::security::{generate_session_token, session_token_digest},
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
    let digest = session_token_digest(email);
    let handle = format!(
        "{display_name}_{:016x}",
        u64::from_be_bytes(digest[..8].try_into().unwrap())
    );
    let form = format!(
        "email={email}&handle={handle}&password=LongSecurePassword2026&confirm_password=LongSecurePassword2026&csrf_token={}",
        csrf(&body)
    );
    let created = send(
        app,
        request("POST", "/register", &form, &csrf_cookie, ORIGIN),
    )
    .await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    assert!(created
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .any(|v| {
            v.to_str()
                .is_ok_and(|v| v.starts_with("tor-dev-session=") && v.contains("Max-Age=2592000"))
        }));
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
    let login = send(app, request("POST", "/login", &form, &csrf_cookie, ORIGIN)).await;
    assert_eq!(login.status(), StatusCode::SEE_OTHER);
    assert!(login.headers().get_all(header::SET_COOKIE).iter().any(|v| {
        v.to_str()
            .is_ok_and(|v| v.starts_with("tor-dev-session=") && v.contains("Max-Age=2592000"))
    }));
    cookie(&login, "tor-dev-session=")
}

async fn session_id(accounts: &PgAccounts, cookie: &str) -> String {
    let token = cookie.split_once('=').expect("session cookie").1;
    let digest = session_token_digest(token);
    let user_id = accounts
        .active_session_user(&digest)
        .await
        .expect("session lookup")
        .expect("live session");
    let sessions = accounts
        .list_active_sessions(user_id, &digest)
        .await
        .expect("session list");
    sessions
        .into_iter()
        .find(|session| session.current)
        .expect("current session in own list")
        .id
}

#[tokio::test]
#[ignore = "Requires isolated TEST_DATABASE_URL PostgreSQL tor_ci"]
async fn a_03_bulk_session_revocation_preserves_current_and_foreign_http_pg() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool().clone());
    let app = tor_web::router_with_postgres_reading_auth(
        catalog.clone(),
        PgReader::new(catalog.pool().clone()),
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let email = format!("bulk-{nonce}%40example.invalid");
    let current = register(&app, &email, "BulkOwner").await;
    let second = login(&app, &email).await;
    let foreign = register(
        &app,
        &format!("bulk-foreign-{nonce}%40example.invalid"),
        "Foreign",
    )
    .await;
    let current_digest = session_token_digest(current.split_once('=').unwrap().1);
    let user = accounts
        .active_session_user(&current_digest)
        .await
        .unwrap()
        .unwrap();
    let ttl = std::num::NonZeroU32::new(3600).unwrap();
    let mut other_digests = Vec::new();
    for index in 0..51 {
        let digest = session_token_digest(&format!("bulk-{nonce}-{index}"));
        accounts.insert_session(user, &digest, ttl).await.unwrap();
        other_digests.push(digest);
    }
    let visible = accounts
        .list_active_sessions(user, &current_digest)
        .await
        .unwrap();
    assert_eq!(visible.len(), 50);
    assert!(visible[0].current, "current device must never be cut off");
    let page = send(
        &app,
        request("GET", "/me/security/sessions", "", &current, ORIGIN),
    )
    .await;
    assert_eq!(page.status(), StatusCode::OK);
    let csrf_cookie = cookie(&page, "tor-dev-csrf=");
    let html = body_of(page).await;
    assert!(html.contains("Завершить все остальные сеансы"));
    assert!(html.contains("Действует до (UTC)"));
    assert!(!html.contains(&current));
    let token = csrf(&html);
    let cookies = format!("{current}; {csrf_cookie}");
    let form =
        format!("session_id=others&current_password=LongSecurePassword2026&csrf_token={token}");
    let wrong = format!("session_id=others&current_password=wrong&csrf_token={token}");
    for (body, jar, origin, status) in [
        (
            form.as_str(),
            current.as_str(),
            ORIGIN,
            StatusCode::FORBIDDEN,
        ),
        (
            form.as_str(),
            cookies.as_str(),
            "https://evil.invalid",
            StatusCode::FORBIDDEN,
        ),
        (
            wrong.as_str(),
            cookies.as_str(),
            ORIGIN,
            StatusCode::UNAUTHORIZED,
        ),
    ] {
        let denied = send(
            &app,
            request("POST", "/me/security/sessions", body, jar, origin),
        )
        .await;
        assert_eq!(denied.status(), status);
    }
    assert_eq!(
        accounts
            .list_active_sessions(user, &current_digest)
            .await
            .unwrap()
            .len(),
        50
    );
    let success = send(
        &app,
        request("POST", "/me/security/sessions", &form, &cookies, ORIGIN),
    )
    .await;
    assert_eq!(success.status(), StatusCode::SEE_OTHER);
    assert_eq!(success.headers()[header::LOCATION], "/me/security/sessions");
    assert!(!success.headers().contains_key(header::SET_COOKIE));
    let remaining = accounts
        .list_active_sessions(user, &current_digest)
        .await
        .unwrap();
    assert_eq!(remaining.len(), 1);
    assert!(remaining[0].current);
    for digest in other_digests {
        assert_eq!(accounts.active_session_user(&digest).await.unwrap(), None);
    }
    for (session, expected) in [
        (&current, StatusCode::OK),
        (&second, StatusCode::SEE_OTHER),
        (&foreign, StatusCode::OK),
    ] {
        assert_eq!(
            send(&app, request("GET", "/me", "", session, ORIGIN))
                .await
                .status(),
            expected
        );
    }
    let repeated = send(
        &app,
        request("POST", "/me/security/sessions", &form, &cookies, ORIGIN),
    )
    .await;
    assert_eq!(repeated.status(), StatusCode::SEE_OTHER);
    let history = accounts.security_history(&current_digest).await.unwrap();
    let bulk_events: Vec<_> = history
        .iter()
        .filter(|event| event.event_kind == "owner_revoke_others")
        .collect();
    assert_eq!(bulk_events.len(), 1);
    assert_eq!(bulk_events[0].revoked_count, Some(52));
    let refreshed = send(&app, navigation("/me", &second)).await;
    assert_eq!(refreshed.status(), StatusCode::SEE_OTHER);
    assert!(!has_session_cookie(&refreshed));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Requires isolated TEST_DATABASE_URL PostgreSQL tor_ci"]
async fn a_03_owner_session_list_revocation_and_csrf_http_pg() {
    tokio::time::timeout(std::time::Duration::from_secs(120), run_case())
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
        .expect("clock")
        .as_nanos();
    let a = format!("sessions-a-{nonce}%40example.invalid");
    let b = format!("sessions-b-{nonce}%40example.invalid");
    let first = register(&app, &a, "OwnerA").await;
    let second = login(&app, &a).await;
    let foreign = register(&app, &b, "OwnerB").await;
    let first_id = session_id(&lookup, &first).await;
    let second_id = session_id(&lookup, &second).await;
    let foreign_id = session_id(&lookup, &foreign).await;

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
    assert!(html.contains("История безопасности"));
    assert!(html.contains("Регистрация аккаунта"));
    assert!(html.contains("Вход в аккаунт"));
    assert!(!html.contains("Отзыв сеанса"));
    assert!(!html.contains("127.0.0.1"), "never display audit IP");
    assert!(html.contains(&first_id));
    assert!(html.contains(&second_id));
    assert!(!html.contains(&foreign_id), "cannot view foreign sessions");
    assert!(!html.contains(&first), "never render raw session cookies");
    let token = csrf(&html);
    let cookies = format!("{first}; {csrf_cookie}");
    let foreign_form = format!(
        "session_id={foreign_id}&current_password=LongSecurePassword2026&csrf_token={token}"
    );

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

    let revoke_other = format!(
        "session_id={second_id}&current_password=LongSecurePassword2026&csrf_token={token}"
    );
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
    // A stolen cookie and CSRF token cannot revoke a live owner's session.
    let stolen =
        format!("session_id={second_id}&current_password=WrongPassword2026&csrf_token={token}");
    let denied = send(
        &app,
        request("POST", "/me/security/sessions", &stolen, &cookies, ORIGIN),
    )
    .await;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        send(&app, request("GET", "/me", "", &second, ORIGIN))
            .await
            .status(),
        StatusCode::OK
    );

    let success = send(
        &app,
        request(
            "POST",
            "/me/security/sessions",
            &revoke_other,
            &cookies,
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(success.status(), StatusCode::SEE_OTHER);
    assert_eq!(success.headers()[header::LOCATION], "/me/security/sessions");
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

    let history = send(
        &app,
        request("GET", "/me/security/sessions", "", &first, ORIGIN),
    )
    .await;
    assert_eq!(history.status(), StatusCode::OK);
    assert_eq!(history.headers()[header::CACHE_CONTROL], "no-store");
    let history = body_of(history).await;
    assert_eq!(history.matches("Отзыв сеанса").count(), 1);
    assert!(history.contains("Завершено сеансов: 1"));
    assert!(!history.contains(&second_id));
    assert!(!history.contains(&foreign_id));
    assert!(!history.contains(&second));
    let other_history = send(
        &app,
        request("GET", "/me/security/sessions", "", &foreign, ORIGIN),
    )
    .await;
    assert_eq!(other_history.status(), StatusCode::OK);
    let other_history = body_of(other_history).await;
    assert!(!other_history.contains("Отзыв сеанса"));
    assert!(!other_history.contains(&first_id));

    let revoke_self =
        format!("session_id={first_id}&current_password=LongSecurePassword2026&csrf_token={token}");
    let own = send(
        &app,
        request(
            "POST",
            "/me/security/sessions",
            &revoke_self,
            &cookies,
            ORIGIN,
        ),
    )
    .await;
    assert_eq!(own.status(), StatusCode::SEE_OTHER);
    assert_eq!(own.headers()[header::LOCATION], "/login");
    assert!(own
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .any(|v| v.to_str().unwrap_or_default().contains("Max-Age=0")));
    assert_eq!(
        send(&app, request("GET", "/me", "", &first, ORIGIN))
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
}

fn navigation(path: &str, cookies: &str) -> Request<Body> {
    let mut req = request("GET", path, "", cookies, ORIGIN);
    for (name, value) in [
        ("sec-fetch-site", "same-origin"),
        ("sec-fetch-mode", "navigate"),
        ("sec-fetch-dest", "document"),
        ("sec-fetch-user", "?1"),
    ] {
        req.headers_mut().insert(name, value.parse().unwrap());
    }
    req
}

fn has_session_cookie(response: &Response) -> bool {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .any(|v| v.to_str().is_ok_and(|v| v.starts_with("tor-dev-session=")))
}

#[tokio::test]
#[ignore = "Requires isolated TEST_DATABASE_URL PostgreSQL tor_ci"]
async fn a_03_rolling_session_browser_navigation_pg() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool().clone());
    let app = tor_web::router_with_postgres_reading_auth(
        catalog.clone(),
        PgReader::new(catalog.pool().clone()),
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let signed_in = register(
        &app,
        &format!("rolling-{nonce}%40example.invalid"),
        "Rolling",
    )
    .await;
    let actor = accounts
        .active_session_user(&session_token_digest(signed_in.split_once('=').unwrap().1))
        .await
        .unwrap()
        .unwrap();
    // A session issued by the previous release has the old shorter TTL.
    let raw = generate_session_token();
    let digest = session_token_digest(&raw);
    accounts
        .insert_session(actor, &digest, std::num::NonZeroU32::new(3600).unwrap())
        .await
        .unwrap();
    let legacy = format!("tor-dev-session={raw}");
    for req in [
        request("GET", "/me", "", &legacy, ORIGIN),
        navigation("/assets/site.css", &legacy),
        navigation("/odmen", &legacy),
        {
            let mut req = navigation("/me", &legacy);
            req.headers_mut()
                .insert("sec-fetch-site", "cross-site".parse().unwrap());
            req
        },
    ] {
        assert!(!has_session_cookie(&send(&app, req).await));
    }
    let renewed = send(&app, navigation("/me", &legacy)).await;
    assert_eq!(renewed.status(), StatusCode::OK);
    assert_eq!(cookie(&renewed, "tor-dev-session="), legacy);
    assert_eq!(renewed.headers()[header::CACHE_CONTROL], "no-store");
    assert!(renewed
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .any(|v| {
            v.to_str()
                .is_ok_and(|v| v.starts_with("tor-dev-session=") && v.contains("Max-Age=2592000"))
        }));
    // A second qualifying request also repairs a missed Set-Cookie without
    // issuing a second token or extending the database lifetime again.
    let again = send(&app, navigation("/me", &legacy)).await;
    assert_eq!(cookie(&again, "tor-dev-session="), legacy);
    accounts.revoke_session(&digest).await.unwrap();
    let denied = send(&app, navigation("/me", &legacy)).await;
    assert_eq!(denied.status(), StatusCode::SEE_OTHER);
    assert!(!has_session_cookie(&denied));
}
