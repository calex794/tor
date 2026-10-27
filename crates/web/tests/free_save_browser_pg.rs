//! Real HTTP owner Save in a published book must immediately expose one
//! immutable free chapter, without a second confirmation request.
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use std::{env, num::NonZeroU32, process::Command};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn pg(url: &str, sql: &str) -> String {
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"));
    let result = Command::new("psql")
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
        .expect("PostgreSQL client is required for isolated PG fixture");
    assert!(
        result.status.success(),
        "fixture failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

fn cookies(session: &str, csrf: &str) -> String {
    format!("tor-dev-session={session}; tor-dev-csrf={csrf}")
}

async fn send(
    app: &Router,
    method: &str,
    path: &str,
    body: &str,
    session: Option<&str>,
    csrf: &str,
    origin: &str,
) -> (StatusCode, String) {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded");
    if let Some(session) = session {
        request = request.header(header::COOKIE, cookies(session, csrf));
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body.to_owned())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2_000_000).await.unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[tokio::test]
#[ignore = "requires isolated PG18 tor_ci"]
async fn browser_save_free_publishes_without_separate_release_and_replay_is_safe() {
    let url = env::var("TEST_DATABASE_URL").expect("PG18 fixture");
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let reader = PgReader::new(pool.clone());
    let accounts = PgAccounts::from_pool(pool.clone());
    let token = generate_session_token();
    let password = hash_password("FreeChapterBrowser2026").unwrap();
    let owner = accounts
        .register_with_session(
            "free-browser-owner@example.invalid",
            "Free browser author",
            &password,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
             VALUES('ci:free-browser-save','ru','Free Browser','public',{owner}) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        reader,
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let csrf = generate_session_token();
    let command = generate_session_token();
    let path = format!("/me/books/{book}/chapters");
    let page_path = format!("/me/books/{book}/chapters/new");
    let (status, html) = send(&app, "GET", &page_path, "", Some(&token), &csrf, ORIGIN).await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("сразу после сохранения"));
    assert!(!html.contains("отдельное подтверждение"));
    let body = format!(
        "number=2&title=New+chapter&body=Immediate%20free%20text&access=free&price_amount=0&command_key={command}&csrf_token={csrf}"
    );
    assert_eq!(
        send(&app, "POST", &path, &body, None, &csrf, ORIGIN)
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        send(
            &app,
            "POST",
            &path,
            &body,
            Some(&token),
            &csrf,
            "https://evil.example"
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let malformed = body.replace(&format!("csrf_token={csrf}"), "csrf_token=wrong");
    assert_eq!(
        send(&app, "POST", &path, &malformed, Some(&token), &csrf, ORIGIN)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, "POST", &path, &body, Some(&token), &csrf, ORIGIN)
            .await
            .0,
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        send(&app, "POST", &path, &body, Some(&token), &csrf, ORIGIN)
            .await
            .0,
        StatusCode::SEE_OTHER,
        "same Save can safely replay"
    );
    let altered = body.replace("Immediate%20free%20text", "Changed%20text");
    assert_eq!(
        send(&app, "POST", &path, &altered, Some(&token), &csrf, ORIGIN)
            .await
            .0,
        StatusCode::CONFLICT
    );
    let row = pg(
        &url,
        &format!(
            "SELECT c.id || '|' || c.visibility || '|' || p.version_id
             FROM chapters c JOIN chapter_publications p ON p.chapter_id=c.id
             WHERE c.book_id={book}"
        ),
    );
    let mut columns = row.split('|');
    let chapter_id: i64 = columns.next().unwrap().parse().unwrap();
    assert_eq!(columns.next(), Some("public"));
    let version_id: i64 = columns.next().unwrap().parse().unwrap();
    assert_eq!(columns.next(), None);
    let event = pg(
        &url,
        &format!(
            "SELECT count(*) FROM chapter_first_release_events
             WHERE chapter_id={chapter_id} AND version_id={version_id}"
        ),
    );
    assert_eq!(event, "1");
    let (status, reader_html) = send(
        &app,
        "GET",
        &format!("/reader/{chapter_id}"),
        "",
        None,
        &csrf,
        ORIGIN,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(reader_html.contains("Immediate free text"));
    pool.close().await;
}
