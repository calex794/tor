//! A-04/A-06/A-07 HTTP scoping and credential API on disposable PostgreSQL.
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
async fn call(
    app: &Router,
    method: &str,
    path: &str,
    cookies: &str,
    origin: &str,
    body: String,
) -> axum::response::Response {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::COOKIE, cookies)
        .header(header::ORIGIN, origin)
        .body(Body::from(body))
        .unwrap();
    req.extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([127, 0, 0, 1], 41906))));
    app.clone().oneshot(req).await.unwrap()
}
async fn json(response: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn a_04_a_06_profile_http_rejects_guest_foreign_actor_csrf_and_stale_revision() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1"));
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(pool.clone());
    let app = tor_web::router_with_postgres_reading_auth(
        tor_books::repositories::PgCatalog::from_pool(pool.clone()),
        tor_reading::repositories::PgReader::from_pool(pool.clone()),
        accounts.clone(),
        false,
        ORIGIN.into(),
    );
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let alice = generate_session_token();
    let bob = generate_session_token();
    let hash = hash_password("OriginalSecurePassword2026").unwrap();
    let ttl = NonZeroU32::new(3600).unwrap();
    let alice_id = accounts
        .register_with_session(
            &format!("http-alice-{nonce}@example.invalid"),
            "Alice",
            &hash,
            &session_token_digest(&alice),
            ttl,
        )
        .await
        .unwrap();
    let bob_id = accounts
        .register_with_session(
            &format!("http-bob-{nonce}@example.invalid"),
            "Bob",
            &hash,
            &session_token_digest(&bob),
            ttl,
        )
        .await
        .unwrap();
    assert_eq!(
        call(&app, "GET", "/api/v1/me", "", ORIGIN, String::new())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let response = call(
        &app,
        "GET",
        "/api/v1/me",
        &format!("tor-dev-session={alice}"),
        ORIGIN,
        String::new(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let csrf_cookie = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|h| h.to_str().unwrap())
        .find(|h| h.starts_with("tor-dev-csrf="))
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .to_owned();
    let p = json(response).await;
    assert_eq!(p["id"], alice_id.to_string());
    assert!(p.get("password_hash").is_none());
    let csrf = p["csrf_token"].as_str().unwrap();
    let cookies = format!("tor-dev-session={alice}; {csrf_cookie}");
    let payload = serde_json::json!({"expected_revision":0,"display_name":"Alice updated","handle":format!("http{alice_id}"),"adult_enabled":true,"csrf_token":csrf});
    assert_eq!(
        call(
            &app,
            "PATCH",
            "/api/v1/me",
            &cookies,
            "https://evil.invalid",
            payload.to_string()
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            "/api/v1/me",
            &format!("tor-dev-session={alice}"),
            ORIGIN,
            payload.to_string()
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    let mut forged = payload.clone();
    forged["user_id"] = serde_json::json!(bob_id.to_string());
    assert_eq!(
        call(
            &app,
            "PATCH",
            "/api/v1/me",
            &cookies,
            ORIGIN,
            forged.to_string()
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            "/api/v1/me",
            &cookies,
            ORIGIN,
            payload.to_string()
        )
        .await
        .status(),
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            "/api/v1/me",
            &cookies,
            ORIGIN,
            payload.to_string()
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        accounts
            .profile(bob_id)
            .await
            .unwrap()
            .unwrap()
            .display_name,
        "Bob"
    );
    let change = serde_json::json!({"current_password":"OriginalSecurePassword2026","new_password":"NewSecurePassword2026","csrf_token":csrf});
    let changed = call(
        &app,
        "POST",
        "/api/v1/me/password",
        &cookies,
        ORIGIN,
        change.to_string(),
    )
    .await;
    assert_eq!(changed.status(), StatusCode::NO_CONTENT);
    assert!(accounts
        .active_session_user(&session_token_digest(&alice))
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        accounts
            .active_session_user(&session_token_digest(&bob))
            .await
            .unwrap(),
        Some(bob_id)
    );
    assert_eq!(
        call(&app, "GET", "/api/v1/me", &cookies, ORIGIN, String::new())
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
}
