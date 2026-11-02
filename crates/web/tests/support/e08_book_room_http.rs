//! E-08/S057 book room HTTP path over disposable PostgreSQL.
use super::{call, pg, text, ORIGIN};
use axum::http::StatusCode;
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

#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_book_room_join_read_send_leave_private_and_csrf() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let hash = hash_password("BookRoomTestingPassword2026").unwrap();
    let mut users = Vec::new();
    let mut cookies = Vec::new();
    let mut tokens = Vec::new();
    for i in 0..3 {
        let session = generate_session_token();
        let csrf = generate_session_token();
        users.push(
            accounts
                .register_with_session(
                    &format!("room-http-{nonce}-{i}@example.invalid"),
                    &format!("Room reader {i}"),
                    &hash,
                    &session_token_digest(&session),
                    NonZeroU32::new(3600).unwrap(),
                )
                .await
                .unwrap(),
        );
        cookies.push(format!("tor-dev-session={session}; tor-dev-csrf={csrf}"));
        tokens.push(csrf);
    }
    // Fixture SQL is executed only by the /tor_ci psql helper. Keep direct
    // database access out of the production web crate and its Cargo manifest.
    let book_id: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility)
             VALUES('ci:e08:http:room:{nonce}','ru','Room <script>title</script>','public')
             RETURNING id"
        ),
    )
    .await
    .parse()
    .unwrap();
    let hidden: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility)
             VALUES('ci:e08:http:hidden:{nonce}','ru','Private room','draft')
             RETURNING id"
        ),
    )
    .await
    .parse()
    .unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let page = format!("/books/{book_id}/room");
    assert_eq!(
        call(&app, "GET", &page, "", "", &[]).await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/books/{hidden}/room"),
            &cookies[0],
            "",
            &[]
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    let first = call(&app, "GET", &page, &cookies[0], "", &[]).await;
    assert_eq!(first.status(), StatusCode::OK);
    assert!(first.headers()["content-security-policy"]
        .to_str()
        .unwrap()
        .contains("script-src 'none'"));
    let html = text(first).await;
    assert!(html.contains("Вступить в чат"));
    assert!(html.contains("Room &lt;script&gt;title&lt;/script&gt;"));
    assert!(!html.contains("Отправить</button>"));
    let join_path = format!("{page}/join");
    let join_form = format!("csrf_token={}", tokens[0]);
    assert_eq!(
        call(
            &app,
            "POST",
            &join_path,
            &cookies[0],
            &join_form,
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
            &join_path,
            &cookies[0],
            "csrf_token=forged",
            &[]
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "POST", &join_path, &cookies[0], &join_form, &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        call(&app, "POST", &join_path, &cookies[0], &join_form, &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let member = text(call(&app, "GET", &page, &cookies[0], "", &[]).await).await;
    assert!(member.contains("Пока сообщений нет."));
    assert!(member.contains("name=\"command_key\""));
    let command_key = member
        .split("name=\"command_key\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    assert_eq!(command_key.len(), 43);
    let payload =
        format!("body=%3Cimg%20src%3Dx%20onerror%3Dalert(1)%3E&command_key={command_key}");
    let body = format!("{payload}&csrf_token={}", tokens[0]);
    let nonmember_body = format!("{payload}&csrf_token={}", tokens[1]);
    let send_path = format!("{page}/messages");
    // A token from another session must fail CSRF before membership lookup.
    assert_eq!(
        call(&app, "POST", &send_path, &cookies[1], &body, &[])
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    // Valid own-session CSRF reaches the repository and denies the outsider.
    assert_eq!(
        call(&app, "POST", &send_path, &cookies[1], &nonmember_body, &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "GET", &page, &cookies[1], "", &[])
            .await
            .status(),
        StatusCode::OK
    );
    assert!(!text(call(&app, "GET", &page, &cookies[1], "", &[]).await)
        .await
        .contains("<img src=x onerror=alert(1)>"));
    for _ in 0..2 {
        assert_eq!(
            call(&app, "POST", &send_path, &cookies[0], &body, &[])
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
    }
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM social_book_room_messages WHERE book_id={book_id}")
        )
        .await,
        "1"
    );
    let readable = text(call(&app, "GET", &page, &cookies[0], "", &[]).await).await;
    assert!(readable.contains("&lt;img src=x onerror=alert(1)&gt;"));
    assert!(!readable.contains("<img src=x onerror=alert(1)>"));
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("{page}/leave"),
            &cookies[0],
            &format!("csrf_token={}", tokens[0]),
            &[]
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let left = text(call(&app, "GET", &page, &cookies[0], "", &[]).await).await;
    assert!(left.contains("Вступить в чат"));
    assert!(!left.contains("&lt;img src=x onerror=alert(1)&gt;"));
    assert_eq!(
        call(&app, "POST", &send_path, &cookies[0], &body, &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "POST", &join_path, &cookies[0], &join_form, &[])
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert!(text(call(&app, "GET", &page, &cookies[0], "", &[]).await)
        .await
        .contains("&lt;img src=x onerror=alert(1)&gt;"));
    pg(
        &url,
        &format!("UPDATE books SET visibility='draft' WHERE id={book_id}"),
    )
    .await;
    assert_eq!(
        call(&app, "GET", &page, &cookies[0], "", &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "POST", &send_path, &cookies[0], &body, &[])
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    // The isolated tor_ci database is discarded by CI.
}
