//! C-08/S007: pending deletion does not erase a reader's private bookshelf.
//! This is NOT a saved free-chapter archive or a completed ordinary deletion.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
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
use tor_reading::repositories::{PgLibrary, PgReader};
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
        .expect("psql");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn call(app: &Router, method: &str, path: &str, cookies: &str, body: &str) -> Response {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header(header::ORIGIN, ORIGIN)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::COOKIE, cookies)
        .body(Body::from(body.to_owned()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

async fn body(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn c_08_pending_deletion_bookmark_survives_in_private_http_and_json() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "no persistent database");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let token = generate_session_token();
    let stranger_token = generate_session_token();
    let csrf = generate_session_token();
    let password = hash_password("C08PendingLibrary2026").unwrap();
    let reader = accounts
        .register_with_session(
            &format!("c08-pending-{nonce}@example.invalid"),
            "Saved reader",
            &password,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let other = accounts
        .register_with_session(
            &format!("c08-other-{nonce}@example.invalid"),
            "Other reader",
            &password,
            &session_token_digest(&stranger_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
             VALUES('ci:c08:pending:{nonce}','ru','Архив <script>', 'public','basic')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let bookmarks = PgLibrary::new(pool.clone());
    bookmarks.add_book(reader, book).await.unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let cookie = format!("tor-dev-session={token}; tor-dev-csrf={csrf}");
    let other_cookie = format!("tor-dev-session={stranger_token}; tor-dev-csrf={csrf}");

    let first = call(&app, "GET", "/me/library", &cookie, "").await;
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(first.headers()[header::CACHE_CONTROL], "no-store");
    let html = body(first).await;
    assert!(!html.contains("Ожидает удаления"));

    pg(
        &url,
        &format!("UPDATE books SET lifecycle_status='pending_deletion' WHERE id={book}"),
    );
    let after = call(&app, "GET", "/me/library", &cookie, "").await;
    assert_eq!(after.status(), StatusCode::OK);
    let page = body(after).await;
    assert!(page.contains("Ожидает удаления"));
    assert!(page.contains(&format!("href=\"/books/{book}\"")));
    assert!(page.contains("Архив &lt;script&gt;"));
    assert!(!page.contains("Архив <script>"));

    let json_response = call(&app, "GET", "/api/v1/me/library", &cookie, "").await;
    assert_eq!(json_response.status(), StatusCode::OK);
    let json = body(json_response).await;
    assert!(json.contains(&format!("\"book_id\":\"{book}\"")));
    assert!(json.contains("\"pending_deletion\":true"));
    let stranger = body(call(&app, "GET", "/me/library", &other_cookie, "").await).await;
    assert!(!stranger.contains("Архив &lt;script&gt;"));
    assert!(!body(call(&app, "GET", "/me/library", "", "").await)
        .await
        .contains("Архив &lt;script&gt;"));
    assert!(other > 0);

    let payload = format!("csrf_token={csrf}");
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/me/library/{book}"),
            &other_cookie,
            &payload
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/me/library/{book}"),
            &cookie,
            &payload
        )
        .await
        .status(),
        StatusCode::NOT_FOUND,
        "existing saved Book cannot be newly added during pending deletion"
    );

    pg(
        &url,
        &format!("UPDATE books SET admin_hold=TRUE WHERE id={book}"),
    );
    let held = body(call(&app, "GET", "/me/library", &cookie, "").await).await;
    assert!(!held.contains("Архив &lt;script&gt;"));
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=FALSE WHERE id={book}"),
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/me/library/{book}/remove"),
            &cookie,
            &payload
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let cleared = body(call(&app, "GET", "/api/v1/me/library", &cookie, "").await).await;
    assert!(!cleared.contains(&format!("\"book_id\":\"{book}\"")));
    pg(&url, &format!("DELETE FROM books WHERE id={book}"));
    pool.close().await;
}
