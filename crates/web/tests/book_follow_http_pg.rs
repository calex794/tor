//! C-15: follow intent stays private to the signed-in reader and never grants access.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use std::{
    num::NonZeroU32,
    process::Command,
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

fn pg(url: &str, sql: &str) -> String {
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let output = Command::new("psql")
        .args([
            "-X",
            "-A",
            "-t",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            url,
            "--command",
            sql,
        ])
        .output()
        .expect("psql client");
    assert!(
        output.status.success(),
        "SQL: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn post(app: &Router, path: &str, cookies: &str, origin: &str, csrf: &str) -> StatusCode {
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header(header::COOKIE, cookies)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(format!("csrf_token={csrf}")))
        .expect("request");
    app.clone()
        .oneshot(request)
        .await
        .expect("response")
        .status()
}

async fn book_page(app: &Router, book_id: i64, cookies: &str) -> String {
    let request = Request::builder()
        .uri(format!("/books/{book_id}"))
        .header(header::COOKIE, cookies)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    String::from_utf8(to_bytes(response.into_body(), 1024 * 1024).await.unwrap().to_vec())
        .unwrap()
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 tor_ci"]
async fn c_15_follow_is_idempotent_actor_scoped_and_not_an_entitlement() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database");
    assert!(url.ends_with("/tor_ci"), "no production");
    let catalog = PgCatalog::connect(&url).await.expect("pool");
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password = hash_password("C15FollowIsolated2026").unwrap();
    let first_token = generate_session_token();
    let second_token = generate_session_token();
    let csrf = generate_session_token();
    let first = accounts
        .register_with_session(
            &format!("c15-follow-1-{nonce}@tor-ci.example"),
            "Reader 1",
            &password,
            &session_token_digest(&first_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let second = accounts
        .register_with_session(
            &format!("c15-follow-2-{nonce}@tor-ci.example"),
            "Reader 2",
            &password,
            &session_token_digest(&second_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let public: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:c15:follow:{nonce}','ru','Public','public',{first}) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let hidden: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES('ci:c15:follow:hidden:{nonce}','ru','Hidden','draft',{first}) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let one = format!("tor-dev-session={first_token}; tor-dev-csrf={csrf}");
    let two = format!("tor-dev-session={second_token}; tor-dev-csrf={csrf}");
    let guest_page = book_page(&app, public, "").await;
    assert!(guest_page.contains("href=\"/login\">Следить за книгой"));
    let initial_page = book_page(&app, public, &one).await;
    assert!(initial_page.contains(&format!("action=\"/me/books/{public}/follow\"")));
    assert!(initial_page.contains(&format!("value=\"{csrf}\"")));
    let follow = format!("/me/books/{public}/follow");
    let unfollow = format!("/me/books/{public}/unfollow");
    for _ in 0..2 {
        assert_eq!(
            post(&app, &follow, &one, ORIGIN, &csrf).await,
            StatusCode::SEE_OTHER
        );
    }
    let subscribed_page = book_page(&app, public, &one).await;
    assert!(subscribed_page.contains(&format!("action=\"/me/books/{public}/unfollow\"")));
    assert!(subscribed_page.contains("Отписаться от книги"));
    assert_eq!(
        pg(
            &url,
            &format!(
        "SELECT count(*) FROM reader_book_follows WHERE book_id={public} AND user_id={first}"
    )
        ),
        "1",
        "replay must not duplicate follows"
    );

    assert_eq!(
        post(&app, &follow, &one, "https://other.invalid", &csrf).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(&app, &follow, &one, ORIGIN, "forged").await,
        StatusCode::FORBIDDEN
    );
    let denied = post(&app, &follow, "", ORIGIN, &csrf).await;
    assert!(matches!(
        denied,
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
    ));
    assert_eq!(
        post(
            &app,
            &format!("/me/books/{hidden}/follow"),
            &one,
            ORIGIN,
            &csrf
        )
        .await,
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        post(&app, &follow, &two, ORIGIN, &csrf).await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM reader_book_follows WHERE book_id={public}")
        ),
        "2"
    );
    assert_eq!(
        post(&app, &unfollow, &one, ORIGIN, &csrf).await,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        post(&app, &unfollow, &one, ORIGIN, &csrf).await,
        StatusCode::SEE_OTHER,
        "unfollow must be idempotent"
    );
    let unsubscribed_page = book_page(&app, public, &one).await;
    assert!(unsubscribed_page.contains(&format!("action=\"/me/books/{public}/follow\"")));
    assert_eq!(
        pg(
            &url,
            &format!("SELECT user_id FROM reader_book_follows WHERE book_id={public}")
        ),
        second.to_string(),
        "one reader cannot delete another reader's follow"
    );
    assert_eq!(
        pg(
            &url,
            &format!(
        "SELECT count(*) FROM reader_chapter_completions WHERE user_id IN ({first},{second})"
    )
        ),
        "0",
        "following a book must not mark chapters read"
    );
    pg(
        &url,
        &format!("DELETE FROM books WHERE id IN ({public},{hidden})"),
    );
    pg(
        &url,
        &format!("DELETE FROM users WHERE id IN ({first},{second})"),
    );
    pool.close().await;
}
