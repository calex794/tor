//! B: NEwBOO book likes, real HTTP sessions, PostgreSQL and CSRF guards.
#![forbid(unsafe_code)]

use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::time::{SystemTime, UNIX_EPOCH};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::{PgBookEngagementIntegrity, PgBookLikes, PgReader};
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

fn pg(url: &str, statement: &str) -> String {
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let result = std::process::Command::new("psql")
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
        .expect("psql installed");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
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
    let req = Request::builder()
        .uri(path)
        .method(method)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

async fn html(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 tor_ci"]
async fn book_like_public_count_browser_csrf_repeat_unlike_moderation_and_limit() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PostgreSQL URL");
    assert!(url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let token_a = generate_session_token();
    let token_b = generate_session_token();
    let token_c = generate_session_token();
    let csrf = generate_session_token();
    let user_a = accounts
        .register_with_session(
            &format!("book-like-http-a-{nonce}@example.invalid"),
            "Liker A",
            &hash_password("BookLikeBrowserPassword2026!").unwrap(),
            &session_token_digest(&token_a),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let user_b = accounts
        .register_with_session(
            &format!("book-like-http-b-{nonce}@example.invalid"),
            "Liker B",
            &hash_password("BookLikeBrowserPassword2026!").unwrap(),
            &session_token_digest(&token_b),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let user_c = accounts
        .register_with_session(
            &format!("book-like-http-c-{nonce}@example.invalid"),
            "Liker C",
            &hash_password("BookLikeBrowserPassword2026!").unwrap(),
            &session_token_digest(&token_c),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility)
             VALUES('ci:likes-http:{nonce}','ru','Заголовок <like>','public') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},1,'Глава 1','public','free',0) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({chapter},1,'Public reading test text') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id)
             VALUES({chapter},{version})"
        ),
    );
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let cookie_a = format!("tor-dev-session={token_a}; tor-dev-csrf={csrf}");
    let cookie_b = format!("tor-dev-session={token_b}; tor-dev-csrf={csrf}");
    let cookie_c = format!("tor-dev-session={token_c}; tor-dev-csrf={csrf}");
    let detail = format!("/books/{book}");
    let like = format!("/books/{book}/like");
    let add = format!("csrf_token={csrf}&liked=1");
    let remove = format!("csrf_token={csrf}&liked=0");
    let likes = PgBookLikes::new(pool.clone());
    let integrity = PgBookEngagementIntegrity::new(pool.clone());

    let public = call(&app, "GET", &detail, "", ORIGIN, "").await;
    assert_eq!(public.status(), StatusCode::OK);
    let public_html = html(public).await;
    assert!(public_html.contains("Нравится: 0"));
    assert!(public_html.contains(&format!("href=\"{like}\"")));
    assert!(!public_html.contains("Заголовок <like>"));
    assert!(public_html.contains("Заголовок &lt;like&gt;"));
    let guest = call(&app, "GET", &like, "", ORIGIN, "").await;
    assert_eq!(guest.status(), StatusCode::SEE_OTHER);
    assert_eq!(guest.headers()[header::LOCATION], "/login");

    assert_eq!(
        call(&app, "POST", &like, &cookie_a, "http://evil.invalid", &add)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &like,
            &cookie_a,
            ORIGIN,
            "liked=1&csrf_token=bad"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &like,
            &cookie_a,
            ORIGIN,
            &format!("csrf_token={csrf}&liked=yes")
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY
    );
    assert_eq!(likes.public_count(book).await.unwrap(), Some(0));

    assert_eq!(
        call(&app, "POST", &like, &cookie_a, ORIGIN, &add)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        call(&app, "POST", &like, &cookie_a, ORIGIN, &add)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(likes.public_count(book).await.unwrap(), Some(1));
    let mine = call(&app, "GET", &like, &cookie_a, ORIGIN, "").await;
    assert_eq!(mine.status(), StatusCode::OK);
    assert_eq!(mine.headers()[header::CACHE_CONTROL], "no-store");
    let mine_html = html(mine).await;
    assert!(mine_html.contains("Отметок «Нравится»: 1"));
    assert!(mine_html.contains("name=\"liked\" value=\"0\""));
    assert!(mine_html.contains("Заголовок &lt;like&gt;"));
    assert!(!mine_html.contains("Заголовок <like>"));
    assert!(!mine_html.contains("user_id"));

    assert_eq!(
        call(&app, "POST", &like, &cookie_a, ORIGIN, &remove)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        call(&app, "POST", &like, &cookie_a, ORIGIN, &remove)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(likes.public_count(book).await.unwrap(), Some(0));
    assert_eq!(
        call(&app, "POST", &like, &cookie_a, ORIGIN, &add)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(likes.public_count(book).await.unwrap(), Some(1));

    assert_eq!(
        call(&app, "POST", &like, &cookie_b, ORIGIN, &add)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    for _ in 0..19 {
        assert_eq!(
            call(&app, "POST", &like, &cookie_b, ORIGIN, &add)
                .await
                .status(),
            StatusCode::SEE_OTHER
        );
    }
    let limited = call(&app, "POST", &like, &cookie_b, ORIGIN, &add).await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.headers()[header::RETRY_AFTER], "60");
    assert_eq!(likes.public_count(book).await.unwrap(), Some(2));
    assert!(html(call(&app, "GET", &detail, "", ORIGIN, "").await)
        .await
        .contains("Нравится: 2"));

    let suspect_book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility)
             VALUES('ci:likes-suspect:{nonce}','ru','Suspicious rate fixture','public')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    for _ in 0..30 {
        integrity
            .evaluate_like(user_c, suspect_book, true)
            .await
            .unwrap();
    }
    let suspect_path = format!("/books/{suspect_book}/like");
    assert_eq!(
        call(&app, "POST", &suspect_path, &cookie_c, ORIGIN, &add)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(likes.public_count(suspect_book).await.unwrap(), Some(0));
    let suspect_summary = likes
        .visible_summary(user_c, suspect_book)
        .await
        .unwrap()
        .unwrap();
    assert!(suspect_summary.liked);
    assert_eq!(suspect_summary.count, 0);
    let suspect_events: i64 = pg(
        &url,
        &format!(
            "SELECT COUNT(*) FROM book_engagement_integrity_events
             WHERE user_id={user_c} AND book_id={suspect_book}"
        ),
    )
    .parse()
    .unwrap();
    assert_eq!(suspect_events, 1);

    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={book}"),
    );
    assert_eq!(likes.public_count(book).await.unwrap(), None);
    assert_eq!(
        call(&app, "GET", &like, &cookie_a, ORIGIN, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "POST", &like, &cookie_a, ORIGIN, &remove)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "GET", &detail, "", ORIGIN, "").await.status(),
        StatusCode::NOT_FOUND
    );
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=false WHERE id={book}"),
    );

    // Reader erasure must not leave ghost popularity.
    pg(&url, &format!("DELETE FROM users WHERE id={user_a}"));
    assert_eq!(likes.public_count(book).await.unwrap(), Some(1));
    pg(&url, &format!("DELETE FROM users WHERE id={user_b}"));
    assert_eq!(likes.public_count(book).await.unwrap(), Some(0));
    pg(&url, &format!("DELETE FROM users WHERE id={user_c}"));
    assert_eq!(likes.public_count(suspect_book).await.unwrap(), Some(0));
    pool.close().await;
}
