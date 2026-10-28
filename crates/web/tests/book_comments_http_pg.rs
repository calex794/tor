//! E-02: browser-style HTTP comments with real disposable PostgreSQL.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::{
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

fn pg(url: &str, statement: &str) -> String {
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
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
            statement,
        ])
        .output()
        .expect("PostgreSQL client");
    assert!(
        result.status.success(),
        "fixture SQL failed: {}",
        String::from_utf8_lossy(&result.stderr),
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    origin: &str,
    body: &str,
) -> Response {
    let request = Request::builder()
        .uri(path)
        .method(method)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .unwrap();
    app.clone().oneshot(request).await.unwrap()
}

async fn html(reply: Response) -> String {
    String::from_utf8(
        to_bytes(reply.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL ending in /tor_ci"]
async fn e_02_http_comment_session_csrf_replay_and_hold() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated test database");
    assert!(url.ends_with("/tor_ci"), "never use production");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let session = generate_session_token();
    let csrf = generate_session_token();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let user = accounts
        .register_with_session(
            &format!("e02-http-{nonce}@tor-ci.example"),
            "Reader",
            &hash_password("E02OnlyTestingPassword2026").unwrap(),
            &session_token_digest(&session),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility)
             VALUES('ci:e02:http:{nonce}','ru','Книга <script>','public') RETURNING id"
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
    let comments_path = format!("/books/{book}/comments");
    let anonymous = call(&app, "GET", &comments_path, "", ORIGIN, "").await;
    assert_eq!(anonymous.status(), StatusCode::OK);
    let anonymous_html = html(anonymous).await;
    assert!(anonymous_html.contains("Войти, чтобы оставить комментарий"));
    assert!(!anonymous_html.contains("name=\"csrf_token\""));

    let detail = html(call(&app, "GET", &format!("/books/{book}"), "", ORIGIN, "").await).await;
    assert!(detail.contains(&format!("href=\"{comments_path}\"")));

    let cookies = format!("tor-dev-session={session}; tor-dev-csrf={csrf}");
    let page = call(&app, "GET", &comments_path, &cookies, ORIGIN, "").await;
    assert_eq!(page.status(), StatusCode::OK);
    let page_html = html(page).await;
    assert!(page_html.contains("Книга &lt;script&gt;"));
    let key = page_html
        .split("name=\"command_key\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap();
    assert_eq!(key.len(), 43);
    let form = format!("body=Hello%20%3Cscript%3E&command_key={key}&csrf_token={csrf}");

    assert_eq!(
        call(&app, "POST", &comments_path, "", ORIGIN, &form)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &comments_path,
            &cookies,
            "https://evil.invalid",
            &form,
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &comments_path,
            &cookies,
            ORIGIN,
            &form.replace(&csrf, "forged"),
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &comments_path,
            &format!("tor-dev-csrf={csrf}"),
            ORIGIN,
            &form,
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    for _ in 0..2 {
        assert_eq!(
            call(&app, "POST", &comments_path, &cookies, ORIGIN, &form)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
    }
    assert_eq!(
        call(
            &app,
            "POST",
            &comments_path,
            &cookies,
            ORIGIN,
            &form.replace("Hello", "Other"),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let visible = html(call(&app, "GET", &comments_path, "", ORIGIN, "").await).await;
    assert!(visible.contains("Hello &lt;script&gt;"));
    assert!(!visible.contains("Hello <script>"));
    let count: i64 = pg(
        &url,
        &format!("SELECT count(*) FROM discussion_comments WHERE book_id={book}"),
    )
    .parse()
    .unwrap();
    assert_eq!(count, 1);

    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={book}"),
    );
    assert_eq!(
        call(&app, "GET", &comments_path, "", ORIGIN, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let held_form = format!(
        "body=Held&command_key={}&csrf_token={csrf}",
        generate_session_token()
    );
    assert_eq!(
        call(&app, "POST", &comments_path, &cookies, ORIGIN, &held_form)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=false WHERE id={book}"),
    );
    pg(&url, &format!("DELETE FROM users WHERE id={user}"));
    pg(&url, &format!("DELETE FROM books WHERE id={book}"));
}
