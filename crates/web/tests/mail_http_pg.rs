//! E-08/S057: real session cookies, HTML forms, JSON, CSRF and private PG data.
use axum::{
    body::{to_bytes, Body},
    http::{header, HeaderName, HeaderValue, Request, StatusCode},
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
use tor_social::repositories::notifications::PgNotifications;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";
async fn call(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    body: &str,
    extra: &[(&str, &str)],
) -> Response {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    for (name, value) in extra {
        builder.headers_mut().unwrap().insert(
            HeaderName::from_bytes(name.as_bytes()).unwrap(),
            HeaderValue::from_str(value).unwrap(),
        );
    }
    app.clone()
        .oneshot(builder.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap()
}
async fn text(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}
#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_http_private_conversations_forms_polling_csrf_and_block() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let hash = hash_password("MailOnlyTestingPassword2026").unwrap();
    let mut users = Vec::new();
    let mut cookies = Vec::new();
    let mut csrf = Vec::new();
    for n in 0..3 {
        let session = generate_session_token();
        let token = generate_session_token();
        let id = accounts
            .register_with_session(
                &format!("mail-http-{nonce}-{n}@example.invalid"),
                &format!("Mail reader {n}"),
                &hash,
                &session_token_digest(&session),
                NonZeroU32::new(3600).unwrap(),
            )
            .await
            .unwrap();
        users.push(id);
        cookies.push(format!("tor-dev-session={session}; tor-dev-csrf={token}"));
        csrf.push(token);
    }
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.into(),
    );
    assert_eq!(
        call(&app, "GET", "/mail", "", "", &[]).await.status(),
        StatusCode::SEE_OTHER
    );
    let index = call(&app, "GET", "/mail", &cookies[0], "", &[]).await;
    assert!(index.headers()[header::CACHE_CONTROL]
        .to_str()
        .unwrap()
        .contains("no-store"));
    let index_html = text(index).await;
    assert!(!index_html.contains("Номер получателя"));
    assert!(!index_html.contains("data-mail-user"));
    let profile = text(
        call(
            &app,
            "GET",
            "/users/Mail%20reader%201",
            &cookies[0],
            "",
            &[],
        )
        .await,
    )
    .await;
    assert!(profile.contains("action=\"/mail/start/"));
    assert!(profile.contains(">Почта</button>"));
    let guest_profile =
        text(call(&app, "GET", "/users/Mail%20reader%201", "", "", &[]).await).await;
    assert!(!guest_profile.contains(">Почта</button>"));
    let start_path = format!("/mail/start/{}", users[1]);
    let start = format!("csrf_token={}", csrf[0]);
    assert_eq!(
        call(
            &app,
            "POST",
            &start_path,
            &cookies[0],
            &start,
            &[("origin", "https://evil.invalid")]
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &start_path,
            &cookies[0],
            &start.replace(&csrf[0], "forged"),
            &[]
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    // NEwBOO friendship: outgoing -> reciprocal acceptance -> removal.
    let request_friend = format!("/users/{}/friend", users[1]);
    let accept_friend = format!("/users/{}/friend", users[0]);
    let remove_friend = format!("/users/{}/unfriend", users[1]);
    assert_eq!(
        call(&app, "POST", &request_friend, &cookies[0], &start, &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let outgoing = text(
        call(
            &app,
            "GET",
            "/users/Mail%20reader%201",
            &cookies[0],
            "",
            &[],
        )
        .await,
    )
    .await;
    assert!(outgoing.contains("Заявка уже отправлена"));
    let incoming = text(
        call(
            &app,
            "GET",
            "/users/Mail%20reader%200",
            &cookies[1],
            "",
            &[],
        )
        .await,
    )
    .await;
    assert!(incoming.contains("Принять в друзья"));
    let accept = format!("csrf_token={}", csrf[1]);
    assert_eq!(
        call(&app, "POST", &accept_friend, &cookies[1], &accept, &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let accepted = text(
        call(
            &app,
            "GET",
            "/users/Mail%20reader%201",
            &cookies[0],
            "",
            &[],
        )
        .await,
    )
    .await;
    assert!(accepted.contains("Удалить из друзей"));
    assert_eq!(
        call(&app, "POST", &remove_friend, &cookies[0], &start, &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let removed = text(
        call(
            &app,
            "GET",
            "/users/Mail%20reader%201",
            &cookies[0],
            "",
            &[],
        )
        .await,
    )
    .await;
    assert!(removed.contains("Добавить в друзья"));
    assert_eq!(
        call(
            &app,
            "POST",
            &request_friend,
            &cookies[0],
            &start,
            &[("origin", "https://attacker.invalid")]
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &request_friend,
            &cookies[0],
            "csrf_token=forged",
            &[]
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &request_friend,
            &cookies[2],
            &format!("csrf_token={}", csrf[2]),
            &[]
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    // No browser-supplied actor field can reassign the verified session identity.
    assert_eq!(
        call(
            &app,
            "POST",
            &request_friend,
            &cookies[0],
            &format!("csrf_token={}&actor_id={}", csrf[0], users[2]),
            &[]
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );

    let response = call(&app, "POST", &start_path, &cookies[0], &start, &[]).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let path = response.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_owned();
    let id: i64 = path.strip_prefix("/mail/").unwrap().parse().unwrap();
    let page = text(call(&app, "GET", &path, &cookies[0], "", &[]).await).await;
    assert!(!page.contains("name=\"command_key\""));
    assert!(page.contains("name=\"client_message_id\""));
    let key = generate_session_token();
    let send_path = format!("{path}/messages");
    let form = format!(
        "body=PRIVATE%0D%0A%3Cscript%3E%20%2Fbooks%2F2&command_key={key}&csrf_token={}",
        csrf[0]
    );
    for _ in 0..2 {
        assert_eq!(
            call(&app, "POST", &send_path, &cookies[0], &form, &[])
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
    }
    // Real recipient-only notification projection, not an AJAX mail read receipt.
    let notifications = PgNotifications::new(pool.clone());
    for _ in 0..12 {
        if notifications.deliver_pending(100).await.unwrap() < 100 {
            break;
        }
    }
    let recipient_rows = notifications.list(users[1], None, 50).await.unwrap();
    let new_mail: Vec<_> = recipient_rows.iter().filter(|r| r.kind == "mail").collect();
    assert_eq!(new_mail.len(), 1, "HTML resend is one notification");
    assert_eq!(new_mail[0].title, "Новое сообщение");
    assert_eq!(new_mail[0].body, "Mail reader 0 написал вам");
    let source_id = new_mail[0].id;
    let list_html = text(call(&app, "GET", "/notifications", &cookies[1], "", &[]).await).await;
    assert!(list_html.contains("Новое сообщение"));
    assert!(list_html.contains("Mail reader 0 написал вам"));
    assert!(!list_html.contains("PRIVATE\\n"));
    assert_eq!(
        call(&app, "GET", "/notifications", "", "", &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let read_path = format!("/notifications/{source_id}/read");
    assert_eq!(
        call(
            &app,
            "POST",
            &read_path,
            &cookies[2],
            &format!("csrf_token={}", csrf[2]),
            &[]
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &read_path,
            &cookies[1],
            "csrf_token=forged",
            &[]
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &read_path,
            &cookies[1],
            &format!("csrf_token={}", csrf[1]),
            &[]
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert!(
        notifications
            .list(users[1], None, 50)
            .await
            .unwrap()
            .iter()
            .find(|r| r.id == source_id)
            .unwrap()
            .is_read
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/notifications/read-all",
            &cookies[1],
            &format!("csrf_token={}", csrf[1]),
            &[]
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert!(notifications
        .list(users[1], None, 50)
        .await
        .unwrap()
        .iter()
        .all(|r| r.is_read));
    assert_eq!(
        call(
            &app,
            "POST",
            &send_path,
            &cookies[0],
            &form.replace("PRIVATE", "OTHER"),
            &[]
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &send_path,
            &cookies[0],
            &form.replace(&csrf[0], "bad"),
            &[]
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "GET", &path, &cookies[2], "", &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &send_path,
            &cookies[2],
            &form.replace(&csrf[0], &csrf[2]),
            &[]
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let api = format!("/api/v1/me/messages?conversation_id={id}&after_id=0&limit=1");
    assert_eq!(
        call(&app, "GET", &api, "", "", &[]).await.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(&app, "GET", &api, &cookies[2], "", &[]).await.status(),
        StatusCode::NOT_FOUND
    );
    let result = call(&app, "GET", &api, &cookies[1], "", &[]).await;
    assert!(result.headers()[header::CACHE_CONTROL]
        .to_str()
        .unwrap()
        .contains("no-store"));
    let json: serde_json::Value = serde_json::from_str(&text(result).await).unwrap();
    assert_eq!(json["messages"].as_array().unwrap().len(), 1);
    assert_eq!(json["messages"][0]["text"], "PRIVATE\n<script> /books/2");
    let api_headers = [
        ("content-type", "application/json"),
        ("x-csrf-token", csrf[0].as_str()),
        ("idempotency-key", key.as_str()),
    ];
    let payload =
        serde_json::json!({"peer_id":users[1].to_string(),"text":"PRIVATE\n<script> /books/2"})
            .to_string();
    let receipt = call(
        &app,
        "POST",
        "/api/v1/me/messages",
        &cookies[0],
        &payload,
        &api_headers,
    )
    .await;
    assert_eq!(receipt.status(), StatusCode::OK);
    assert!(text(receipt).await.contains("\"replayed\":true"));
    let forged=serde_json::json!({"peer_id":users[1].to_string(),"text":"PRIVATE\n<script> /books/2","actor_id":users[2].to_string()}).to_string();
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/me/messages",
            &cookies[0],
            &forged,
            &api_headers
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/me/messages",
            &cookies[0],
            &payload,
            &[
                ("content-type", "application/json"),
                ("x-csrf-token", &csrf[0])
            ]
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let rendered = text(call(&app, "GET", &path, &cookies[1], "", &[]).await).await;
    assert!(rendered.contains("PRIVATE\n&lt;script&gt; /books/2"));
    assert!(!rendered.contains("PRIVATE <script>"));
    assert!(!rendered.contains("href=\"/books/2"));
    assert!(!rendered.contains("/read\""));
    let inbox: serde_json::Value = serde_json::from_str(
        &text(call(&app, "GET", "/api/v1/me/messages", &cookies[1], "", &[]).await).await,
    )
    .unwrap();
    assert!(inbox["conversations"][0].get("unread").is_none());
    let html_client_form = format!(
        "body=Source%20HTML%20identity&client_message_id=html%3Atest.1&csrf_token={}",
        csrf[0]
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &send_path,
            &cookies[0],
            &html_client_form,
            &[]
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &send_path,
            &cookies[0],
            &html_client_form.replace("Source", "Changed"),
            &[]
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM social_messages WHERE conversation_id={id}")
        )
        .await,
        "2"
    );
    // No-JS HTML has no client identity: retry within five seconds reuses
    // the original message, but the other sender's identical text is distinct.
    let unkeyed = format!("body=NoJS+repeat&client_message_id=&csrf_token={}", csrf[0]);
    for _ in 0..2 {
        assert_eq!(
            call(&app, "POST", &send_path, &cookies[0], &unkeyed, &[])
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
    }
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM social_messages WHERE conversation_id={id}")
        )
        .await,
        "3"
    );
    let sender_b = format!("body=NoJS+repeat&client_message_id=&csrf_token={}", csrf[1]);
    assert_eq!(
        call(&app, "POST", &send_path, &cookies[1], &sender_b, &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM social_messages WHERE conversation_id={id}")
        )
        .await,
        "4"
    );
    let other_client = format!(
        "body=Source%20B&client_message_id=html%3Atest.1&csrf_token={}",
        csrf[1]
    );
    assert_eq!(
        call(&app, "POST", &send_path, &cookies[1], &other_client, &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM social_messages WHERE conversation_id={id}")
        )
        .await,
        "5"
    );
    let blocked = format!("blocked=true&csrf_token={}", csrf[1]);
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{path}/block"),
            &cookies[1],
            &blocked,
            &[]
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let blocked_start = call(&app, "POST", &start_path, &cookies[0], &start, &[]).await;
    assert_eq!(blocked_start.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        blocked_start.headers()[header::LOCATION],
        "/mail?error=blocked"
    );
    let mailbox_error =
        text(call(&app, "GET", "/mail?error=blocked", &cookies[0], "", &[]).await).await;
    assert!(mailbox_error.contains("Переписка заблокирована"));
    assert_eq!(
        call(&app, "POST", &send_path, &cookies[0], &form, &[])
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            "/api/v1/me/messages",
            &cookies[0],
            &payload,
            &api_headers
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{path}/block"),
            &cookies[2],
            &blocked.replace(&csrf[1], &csrf[2]),
            &[]
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert!(text(call(&app, "GET", &path, &cookies[0], "", &[]).await)
        .await
        .contains("Новые сообщения заблокированы"));
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM social_messages WHERE conversation_id={id}")
        )
        .await,
        "5"
    );
    // A fresh HTTP router still reads the committed transcript from PostgreSQL.
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let reopened_pool = catalog.pool().clone();
    let reopened = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(reopened_pool.clone()),
        PgAccounts::from_pool(reopened_pool),
        false,
        ORIGIN.into(),
    );
    assert!(
        text(call(&reopened, "GET", &path, &cookies[1], "", &[]).await)
            .await
            .contains("PRIVATE\n&lt;script&gt;")
    );
    pg(
        &url,
        &format!(
            "DELETE FROM users WHERE id IN({},{},{})",
            users[0], users[1], users[2]
        ),
    )
    .await;
}

// Keep fixture SQL outside production web code. Never target a retained DB.
async fn pg(url: &str, statement: &str) -> String {
    assert!(url.ends_with("/tor_ci"));
    let url = url.to_owned();
    let statement = statement.to_owned();
    tokio::task::spawn_blocking(move || {
        let result = std::process::Command::new("psql")
            .env(
                "PGOPTIONS",
                "-c statement_timeout=10000 -c lock_timeout=5000",
            )
            .env("PGCONNECT_TIMEOUT", "5")
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
                &statement,
            ])
            .output()
            .expect("fixture psql");
        assert!(
            result.status.success(),
            "fixture failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        String::from_utf8(result.stdout).unwrap().trim().to_owned()
    })
    .await
    .unwrap()
}
