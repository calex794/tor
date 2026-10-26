//! B-03/B-06: real HTTP form -> immutable PG chapter -> deliberate publication.
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::num::NonZeroU32;
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, session_token_digest},
};
use tor_books::{
    repositories::{postgres::PgAuthoring, PgCatalog},
    services::authoring::new_book,
};
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

async fn send(app: &Router, method: &str, path: &str, form: &str, cookies: &str, origin: &str) -> Response {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, cookies)
        .header(header::ORIGIN, origin)
        .body(Body::from(form.to_owned()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

async fn response_body(reply: Response) -> String {
    String::from_utf8(to_bytes(reply.into_body(), 128 * 1024).await.unwrap().to_vec()).unwrap()
}

fn csrf(html: &str) -> String {
    html.split("name=\"csrf_token\" value=\"")
        .nth(1).unwrap().split('"').next().unwrap().to_owned()
}

fn command_key(html: &str) -> String {
    html.split("name=\"command_key\" value=\"")
        .nth(1).unwrap().split('"').next().unwrap().to_owned()
}

fn cookie(reply: &Response, prefix: &str) -> String {
    reply.headers().get_all(header::SET_COOKIE).iter()
        .find_map(|value| {
            let s = value.to_str().ok()?;
            s.starts_with(prefix).then(|| s.split(';').next().unwrap_or("").to_owned())
        }).expect("csrf cookie on private form")
}

#[tokio::test]
#[ignore = "requires disposable TEST_DATABASE_URL ending in /tor_ci"]
async fn b_03_06_browser_creates_private_chapter_then_releases() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "Never use production");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let reader = PgReader::new(pool.clone());
    reader.probe().await.unwrap();
    let accounts = PgAccounts::from_pool(pool.clone());
    accounts.probe().await.unwrap();

    let owner = accounts.insert_user(
        "b03-ui-owner@tor-ci.example", "B03 UI Owner", "$argon2id$test"
    ).await.unwrap();
    let outsider = accounts.insert_user(
        "b03-ui-other@tor-ci.example", "B03 UI Other", "$argon2id$test"
    ).await.unwrap();

    let author = PgAuthoring::new(pool.clone());
    let request = new_book(
        &format!("tor-ci:b03-ui:{owner}"), "ru", "Пример книги", &"b".repeat(43)
    ).unwrap();
    let book_id = author.create_owned_book(owner, &request).await.unwrap();
    let path = format!("/me/books/{book_id}/chapters/new");
    let release_path = format!("/me/books/{book_id}/release");

    let token = generate_session_token();
    let digest = session_token_digest(&token);
    accounts.insert_session(owner, &digest, NonZeroU32::new(3600).unwrap()).await.unwrap();
    let session = format!("tor-dev-session={token}");
    let other_token = generate_session_token();
    let other_digest = session_token_digest(&other_token);
    accounts.insert_session(outsider, &other_digest, NonZeroU32::new(3600).unwrap()).await.unwrap();
    let other_session = format!("tor-dev-session={other_token}");

    let app = tor_web::router_with_postgres_reading_auth(
        catalog, reader, accounts, false, ORIGIN.to_owned()
    );
    assert_eq!(send(&app, "GET", &path, "", "", ORIGIN).await.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        send(&app, "GET", &path, "", &other_session, ORIGIN).await.status(),
        StatusCode::NOT_FOUND
    );
    let form_reply = send(&app, "GET", &path, "", &session, ORIGIN).await;
    assert_eq!(form_reply.status(), StatusCode::OK);
    assert_eq!(form_reply.headers()[header::CACHE_CONTROL], "no-store");
    let csrf_cookie = cookie(&form_reply, "tor-dev-csrf=");
    let html = response_body(form_reply).await;
    assert!(html.contains("Текст главы"));
    let token_field = csrf(&html);
    let key = command_key(&html);
    assert_eq!(key.len(), 43);
    let cookies = format!("{session}; {csrf_cookie}");
    let form = format!(
        "number=1&title=Chapter+one&body=Safe+Markdown+text&command_key={key}&csrf_token={token_field}"
    );
    assert_eq!(
        send(&app, "POST", &path, &form, &session, ORIGIN).await.status(),
        StatusCode::FORBIDDEN,
        "missing CSRF cookie"
    );
    assert_eq!(
        send(&app, "POST", &path, &form, &cookies, "https://evil.example").await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "POST", &path, &form, &format!("{other_session}; {csrf_cookie}"), ORIGIN)
            .await.status(),
        StatusCode::NOT_FOUND,
        "another account may not write this book"
    );
    let before = send(
        &app, "GET", &format!("/books/{book_id}"), "", "", ORIGIN
    ).await;
    assert_eq!(before.status(), StatusCode::NOT_FOUND);

    let saved = send(&app, "POST", &path, &form, &cookies, ORIGIN).await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    assert_eq!(saved.headers()[header::LOCATION], release_path);
    let replay = send(&app, "POST", &path, &form, &cookies, ORIGIN).await;
    assert_eq!(replay.status(), StatusCode::SEE_OTHER);
    let changed = form.replace("Safe+Markdown+text", "Different+Markdown+text");
    assert_eq!(
        send(&app, "POST", &path, &changed, &cookies, ORIGIN).await.status(),
        StatusCode::CONFLICT
    );
    let chapter_id: i64 = sqlx::query_scalar(
        "SELECT id FROM chapters WHERE book_id=$1 AND chapter_no=1"
    ).bind(book_id).fetch_one(&pool).await.unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_versions WHERE chapter_id=$1"
    ).bind(chapter_id).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        send(&app, "GET", &format!("/reader/{chapter_id}"), "", "", ORIGIN)
            .await.status(),
        StatusCode::NOT_FOUND
    );

    assert_eq!(
        send(&app, "GET", &release_path, "", &other_session, ORIGIN)
            .await.status(),
        StatusCode::NOT_FOUND
    );
    let release_page = send(&app, "GET", &release_path, "", &cookies, ORIGIN).await;
    assert_eq!(release_page.status(), StatusCode::OK);
    let release_html = response_body(release_page).await;
    assert!(release_html.contains("Опубликовать книгу?"));
    let release_body = format!("csrf_token={}", csrf(&release_html));
    assert_eq!(
        send(&app, "POST", &release_path, &release_body, &session, ORIGIN)
            .await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "POST", &release_path, &release_body, &cookies, "https://evil.example")
            .await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "POST", &release_path, &release_body, &cookies, ORIGIN)
            .await.status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        send(&app, "POST", &release_path, &release_body, &cookies, ORIGIN)
            .await.status(),
        StatusCode::SEE_OTHER,
        "release must be idempotent"
    );
    assert_eq!(
        send(&app, "GET", &format!("/books/{book_id}"), "", "", ORIGIN)
            .await.status(),
        StatusCode::OK
    );
    let chapter_html = response_body(
        send(&app, "GET", &format!("/reader/{chapter_id}"), "", "", ORIGIN).await
    ).await;
    assert!(chapter_html.contains("Safe Markdown text"));
    let releases: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM chapter_first_release_events WHERE chapter_id=$1"
    ).bind(chapter_id).fetch_one(&pool).await.unwrap();
    assert_eq!(releases, 1);
    pool.close().await;
}
