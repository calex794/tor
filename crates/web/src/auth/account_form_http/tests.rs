use super::*;
use axum::body::to_bytes;

#[test]
fn successful_dialog_and_standalone_responses_keep_private_cookies() {
    let cookie = String::from("session=test; HttpOnly; SameSite=Strict");
    let mut headers = HeaderMap::new();
    let plain = auth_success(&headers, Some("/reader/42"), std::slice::from_ref(&cookie));
    assert_eq!(plain.status(), StatusCode::SEE_OTHER);
    assert_eq!(plain.headers()[header::LOCATION], "/reader/42");
    headers.insert("x-tor-auth-dialog", HeaderValue::from_static("1"));
    let dialog = auth_success(&headers, Some("/reader/42"), &[cookie]);
    assert_eq!(dialog.status(), StatusCode::NO_CONTENT);
    assert_eq!(dialog.headers()["x-tor-auth-dialog"], "success");
    assert_eq!(dialog.headers()[header::CACHE_CONTROL], "no-store");
    assert!(dialog.headers().contains_key(header::SET_COOKIE));
    assert!(!dialog.headers().contains_key(header::LOCATION));
}

#[tokio::test]
async fn too_many_login_attempts_has_clear_429_and_retry_after() {
    let reply = auth_limit_error(StatusCode::TOO_MANY_REQUESTS, false);
    assert_eq!(reply.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(reply.headers()[header::RETRY_AFTER], "600");
    let body = to_bytes(reply.into_body(), 65_536).await.expect("HTML");
    let html = String::from_utf8(body.to_vec()).expect("UTF-8");
    assert!(html.contains("Слишком много попыток входа"));
    assert!(html.contains("10 минут"));
    assert!(!html.contains("password_hash"));
}

#[tokio::test]
async fn registration_rate_limit_is_also_honest() {
    let reply = auth_limit_error(StatusCode::TOO_MANY_REQUESTS, true);
    assert_eq!(reply.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(reply.headers()[header::RETRY_AFTER], "600");
}

#[tokio::test]
async fn unavailable_storage_is_not_misreported_as_throttling() {
    let reply = auth_limit_error(StatusCode::SERVICE_UNAVAILABLE, false);
    assert_eq!(reply.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert!(reply.headers().get(header::RETRY_AFTER).is_none());
    let html = to_bytes(reply.into_body(), 65_536).await.expect("HTML");
    assert!(String::from_utf8_lossy(&html).contains("Вход временно недоступен"));
}
