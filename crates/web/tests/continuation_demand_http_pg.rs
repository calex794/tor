//! C-15 / Loop 1: real sessions, CSRF, verified completion and author demand.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use sqlx::PgPool;
use std::time::{SystemTime, UNIX_EPOCH};
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
    cookies: &str,
    origin: &str,
    body: &str,
) -> Response {
    let req = Request::builder()
        .uri(path)
        .method(method)
        .header(header::COOKIE, cookies)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .expect("HTTP request");
    app.clone().oneshot(req).await.expect("HTTP response")
}

async fn html(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 256 * 1024)
            .await
            .expect("HTML body")
            .to_vec(),
    )
    .expect("UTF-8")
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn c_15_http_demand_requires_completion_csrf_and_owner_scoping() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database");
    assert!(url.ends_with("/tor_ci"), "never run against production");
    let catalog = PgCatalog::connect(&url).await.expect("test pool");
    let pool: PgPool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let owner_session = generate_session_token();
    let reader_session = generate_session_token();
    let csrf = generate_session_token();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let password_hash = hash_password("C15ContinuationBrowser2026").unwrap();
    let owner = accounts
        .register_with_session(
            &format!("c15-owner-{nonce}@tor-ci.example"),
            "Author",
            &password_hash,
            &session_token_digest(&owner_session),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let reader = accounts
        .register_with_session(
            &format!("c15-reader-{nonce}@tor-ci.example"),
            "Reader",
            &password_hash,
            &session_token_digest(&reader_session),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();

    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','Книга для читателей','public',$2) RETURNING id",
    )
    .bind(format!("c15:http:{nonce}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Глава 1','public','free',0) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Первая глава') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(&pool)
        .await
        .unwrap();

    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let owner_cookie = format!("tor-dev-session={owner_session}; tor-dev-csrf={csrf}");
    let reader_cookie = format!("tor-dev-session={reader_session}; tor-dev-csrf={csrf}");
    let read_path = format!("/read/{chapter}");
    let want_path = format!("/read/{chapter}/want-continuation");
    let owner_path = format!("/me/books/{book}");
    let form = format!("csrf_token={csrf}");

    let page = call(&app, "GET", &read_path, &reader_cookie, ORIGIN, "").await;
    assert_eq!(page.status(), StatusCode::OK);
    assert!(!html(page).await.contains("Хочу продолжение"));
    assert_eq!(
        call(&app, "POST", &want_path, &reader_cookie, ORIGIN, &form)
            .await
            .status(),
        StatusCode::CONFLICT,
        "opened chapter is not a trusted completion"
    );

    sqlx::query(
        "INSERT INTO reader_chapter_completions
           (user_id,book_id,chapter_id,first_version_id,source_event_id)
         VALUES($1,$2,$3,$4,gen_random_uuid())",
    )
    .bind(reader)
    .bind(book)
    .bind(chapter)
    .bind(version)
    .execute(&pool)
    .await
    .unwrap();

    let ready = html(call(&app, "GET", &read_path, &reader_cookie, ORIGIN, "").await).await;
    assert!(ready.contains("Хочу продолжение"));
    assert!(!ready.contains("user_id"));
    assert_eq!(
        call(
            &app,
            "POST",
            &want_path,
            &reader_cookie,
            "https://evil.invalid",
            &form
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &want_path,
            &reader_cookie,
            ORIGIN,
            "csrf_token=invalid"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "POST", &want_path, "", ORIGIN, &form)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    for _ in 0..2 {
        assert_eq!(
            call(&app, "POST", &want_path, &reader_cookie, ORIGIN, &form)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM reader_continuation_demand
         WHERE user_id=$1 AND book_id=$2 AND frontier_chapter_id=$3",
    )
    .bind(reader)
    .bind(book)
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "duplicate requests must be idempotent");
    let confirmed = html(call(&app, "GET", &read_path, &reader_cookie, ORIGIN, "").await).await;
    assert!(confirmed.contains("Вы уже сообщили автору"));

    assert_eq!(
        call(&app, "GET", &owner_path, &reader_cookie, ORIGIN, "")
            .await
            .status(),
        StatusCode::NOT_FOUND,
        "reader must not see the author's demand counter"
    );
    let author = html(call(&app, "GET", &owner_path, &owner_cookie, ORIGIN, "").await).await;
    assert!(author.contains("Запросов после последней опубликованной главы: <strong>1</strong>"));
    assert!(!author.contains(&format!("c15-reader-{nonce}@tor-ci.example")));

    let next: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,2,'Продолжение','public','paid',150) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let paid_version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'PAID-CONTENT-SECRET') RETURNING id",
    )
    .bind(next)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(next)
        .bind(paid_version)
        .execute(&pool)
        .await
        .unwrap();
    let after_release = html(call(&app, "GET", &read_path, &reader_cookie, ORIGIN, "").await).await;
    assert!(!after_release.contains("Хочу продолжение"));
    assert!(!after_release.contains("PAID-CONTENT-SECRET"));
    assert_eq!(
        call(&app, "POST", &want_path, &reader_cookie, ORIGIN, &form)
            .await
            .status(),
        StatusCode::CONFLICT
    );
    let owner_after = html(call(&app, "GET", &owner_path, &owner_cookie, ORIGIN, "").await).await;
    assert!(
        owner_after.contains("Запросов после последней опубликованной главы: <strong>0</strong>")
    );

    sqlx::query("DELETE FROM books WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1 OR id=$2")
        .bind(reader)
        .bind(owner)
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}
