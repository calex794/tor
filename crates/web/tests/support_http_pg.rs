//! S050/S051: real cookies, guest claim, live admin-role revocation and no leaks.
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
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
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";
async fn call(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    body: &str,
    origin: &str,
) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, origin)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn json_call(app: &Router, cookie: &str, csrf: &str, key: &str, body: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/support/tickets")
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, ORIGIN)
                .header(header::CONTENT_TYPE, "application/json")
                .header("x-csrf-token", csrf)
                .header("idempotency-key", key)
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}
async fn text(r: Response) -> String {
    String::from_utf8(to_bytes(r.into_body(), 128 * 1024).await.unwrap().to_vec()).unwrap()
}
fn issued(r: &Response) -> String {
    r.headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|c| c.to_str().unwrap().split(';').next().unwrap().to_owned())
        .collect::<Vec<_>>()
        .join("; ")
}
async fn pg(url: &str, sql: String) -> String {
    assert!(url.ends_with("/tor_ci"));
    let url = url.to_owned();
    tokio::task::spawn_blocking(move || {
        let r = std::process::Command::new("psql")
            .args([
                "-X",
                "-A",
                "-t",
                "-q",
                "-v",
                "ON_ERROR_STOP=1",
                "--dbname",
                &url,
                "--command",
                &sql,
            ])
            .output()
            .unwrap();
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        String::from_utf8(r.stdout).unwrap().trim().to_owned()
    })
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_11_http_guest_claim_and_live_admin_boundaries() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let hash = hash_password("SupportOnlyPassword2026").unwrap();
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut users = Vec::new();
    let mut sessions = Vec::new();
    let mut tokens = Vec::new();
    for i in 0..3 {
        let session = generate_session_token();
        let csrf = generate_session_token();
        users.push(
            accounts
                .register_with_session(
                    &format!("support-http-{n}-{i}@example.invalid"),
                    "Support fixture",
                    &hash,
                    &session_token_digest(&session),
                    NonZeroU32::new(3600).unwrap(),
                )
                .await
                .unwrap(),
        );
        sessions.push(format!("tor-dev-session={session}; tor-dev-csrf={csrf}"));
        tokens.push(csrf);
    }
    pg(
        &url,
        format!(
            "INSERT INTO account_roles(user_id,role,is_active) VALUES({},'author',true)",
            users[0]
        ),
    )
    .await;
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.into(),
    );
    let widget = call(&app, "GET", "/support/widget", "", "", ORIGIN).await;
    assert_eq!(widget.status(), StatusCode::OK);
    assert_eq!(widget.headers()[header::CACHE_CONTROL], "no-store");
    let guest = issued(&widget);
    let payload: serde_json::Value = serde_json::from_str(&text(widget).await).unwrap();
    let csrf = payload["csrf_token"].as_str().unwrap();
    let chat = payload["conversation_id"]
        .as_str()
        .unwrap()
        .parse::<i64>()
        .unwrap();
    let form = format!("csrf_token={csrf}&body=PRIVATE%20%3Cscript%3E&client_message_id=one");
    assert_eq!(
        call(
            &app,
            "POST",
            "/support/messages",
            &guest,
            &form,
            "https://evil.invalid"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/support/messages",
            &guest,
            "csrf_token=wrong&body=oops",
            ORIGIN
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/support/messages",
            &guest,
            &format!("{form}&requester_user_id={}", users[1]),
            ORIGIN
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    for _ in 0..2 {
        assert_eq!(
            call(&app, "POST", "/support/messages", &guest, &form, ORIGIN)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
    }
    let page = text(call(&app, "GET", "/support", &guest, "", ORIGIN).await).await;
    assert!(page.contains("PRIVATE &lt;script&gt;"));
    assert!(!page.contains("PRIVATE <script>"));
    let outsider = text(call(&app, "GET", "/support/widget", "", "", ORIGIN).await).await;
    assert!(!outsider.contains("PRIVATE"));
    assert_eq!(
        call(&app, "GET", "/admin/support", "", "", ORIGIN)
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "GET", "/admin/support", &sessions[0], "", ORIGIN)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    pg(
        &url,
        format!(
            "INSERT INTO account_roles(user_id,role,is_active) VALUES({},'administrator',true)",
            users[2]
        ),
    )
    .await;
    let list = text(call(&app, "GET", "/admin/support", &sessions[2], "", ORIGIN).await).await;
    assert!(list.contains("PRIVATE &lt;script&gt;"));
    let path = format!("/admin/support/{chat}");
    let detail = text(call(&app, "GET", &path, &sessions[2], "", ORIGIN).await).await;
    assert!(detail.contains("PRIVATE &lt;script&gt;"));
    let reply = format!(
        "csrf_token={}&body=ADMIN%20ANSWER&client_message_id=one",
        tokens[2]
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{path}/messages"),
            &sessions[0],
            &reply,
            ORIGIN
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{path}/messages"),
            &sessions[2],
            &reply,
            "https://evil.invalid"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{path}/messages"),
            &sessions[2],
            &reply,
            ORIGIN
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert!(text(
        call(
            &app,
            "GET",
            "/support/messages?after_id=0",
            &guest,
            "",
            ORIGIN
        )
        .await
    )
    .await
    .contains("ADMIN ANSWER"));
    // Preserve existing user history while claiming the same guest transcript.
    let prior = format!(
        "csrf_token={}&body=EXISTING%20HISTORY&client_message_id=one",
        tokens[0]
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/support/messages",
            &sessions[0],
            &prior,
            ORIGIN
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let capability = guest
        .split("; ")
        .find(|c| c.starts_with("tor-dev-support="))
        .unwrap();
    let claimed_cookie = format!("{}; {capability}", sessions[0]);
    let claim = call(&app, "GET", "/support/widget", &claimed_cookie, "", ORIGIN).await;
    assert!(claim
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .any(|c| c.to_str().unwrap().contains("tor-dev-support=;")));
    let claimed = text(claim).await;
    assert!(claimed.contains("EXISTING HISTORY"));
    assert!(claimed.contains("PRIVATE"));
    assert!(claimed.contains("ADMIN ANSWER"));
    assert_eq!(
        call(
            &app,
            "GET",
            "/support/widget",
            &format!("{}; {capability}", sessions[1]),
            "",
            ORIGIN
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "GET", "/support/widget", &guest, "", ORIGIN)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert!(
        text(call(&app, "GET", "/support/widget", &sessions[0], "", ORIGIN).await)
            .await
            .contains("PRIVATE")
    );
    let key = "K".repeat(43);
    let first = json_call(
        &app,
        &sessions[0],
        &tokens[0],
        &key,
        r#"{"message":"JSON support"}"#,
    )
    .await;
    assert_eq!(first.status(), StatusCode::OK);
    let first: serde_json::Value = serde_json::from_str(&text(first).await).unwrap();
    let again = json_call(
        &app,
        &sessions[0],
        &tokens[0],
        &key,
        r#"{"message":"JSON support"}"#,
    )
    .await;
    let again: serde_json::Value = serde_json::from_str(&text(again).await).unwrap();
    assert_eq!(
        first["message"]["message_id"],
        again["message"]["message_id"]
    );
    assert_eq!(
        json_call(
            &app,
            &sessions[0],
            &tokens[0],
            &key,
            r#"{"message":"different"}"#
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        json_call(&app, &sessions[0], "wrong", &key, r#"{"message":"test"}"#)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        json_call(&app, &sessions[0], &tokens[0], "", r#"{"message":"test"}"#)
            .await
            .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        json_call(
            &app,
            &sessions[0],
            &tokens[0],
            &key,
            r#"{"message":"test","user_id":"1"}"#
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // Revocation denies both fresh polls and existing editor forms immediately.
    pg(
        &url,
        format!(
            "UPDATE account_roles SET is_active=false WHERE user_id={} AND role='administrator'",
            users[2]
        ),
    )
    .await;
    for endpoint in [&path, &format!("{path}/messages")] {
        assert_eq!(
            call(&app, "GET", endpoint, &sessions[2], "", ORIGIN)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{path}/messages"),
            &sessions[2],
            &reply,
            ORIGIN
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    pg(
        &url,
        format!(
            "DELETE FROM users WHERE id IN({},{},{})",
            users[0], users[1], users[2]
        ),
    )
    .await;
}
