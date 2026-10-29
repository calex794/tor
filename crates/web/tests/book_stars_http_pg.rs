//! F-05: authenticated browser rating path using real disposable PostgreSQL.
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

async fn html(result: Response) -> String {
    String::from_utf8(
        to_bytes(result.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 tor_ci"]
async fn f_05_http_reader_ratings_work_translation_csrf_and_moderation() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test PG only");
    assert!(url.ends_with("/tor_ci"));
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
            &format!("f05-stars-http-{nonce}@tor-ci.example"),
            "Reader",
            &hash_password("F05RatingsBrowserPassword2026").unwrap(),
            &session_token_digest(&session),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility)
             VALUES('ci:f05:http:{nonce}','ru','Исходная книга <tag>','public')
             RETURNING id",
        ),
    )
    .parse()
    .unwrap();
    let chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},1,'Глава 1','public','free',0) RETURNING id",
        ),
    )
    .parse()
    .unwrap();
    let version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({chapter},1,'Проверенный текст') RETURNING id",
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id)
             VALUES({chapter},{version})",
        ),
    );
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let cookie = format!("tor-dev-session={session}; tor-dev-csrf={csrf}");
    let rate = format!("/me/books/{book}/rate");
    let detail = format!("/books/{book}");
    let form = format!("csrf_token={csrf}&rating_kind=work&rating=5");

    let unverified = call(&app, "GET", &rate, &cookie, ORIGIN, "").await;
    assert_eq!(unverified.status(), StatusCode::OK);
    assert!(html(unverified).await.contains("Оценка будет доступна"));
    assert_eq!(
        call(&app, "POST", &rate, &cookie, ORIGIN, &form)
            .await
            .status(),
        StatusCode::FORBIDDEN,
        "a mere opened chapter does not permit a trusted rating",
    );
    pg(
        &url,
        &format!(
            "INSERT INTO reader_chapter_completions
                (user_id,book_id,chapter_id,first_version_id,source_event_id)
             VALUES({user},{book},{chapter},{version},gen_random_uuid())",
        ),
    );

    let page = call(&app, "GET", &rate, &cookie, ORIGIN, "").await;
    assert_eq!(page.status(), StatusCode::OK);
    let markup = html(page).await;
    assert!(markup.contains("Произведение") && markup.contains("Перевод"));
    assert!(markup.contains("value=\"translation\""));
    assert!(
        !markup.contains("Исходная книга <tag>"),
        "HTML title must be escaped"
    );
    assert!(markup.contains("Исходная книга &lt;tag&gt;"));

    assert_eq!(
        call(&app, "POST", &rate, &cookie, "http://evil.invalid", &form)
            .await
            .status(),
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &rate,
            &cookie,
            ORIGIN,
            "csrf_token=forged&rating_kind=work&rating=5"
        )
        .await
        .status(),
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &rate,
            &cookie,
            ORIGIN,
            &format!("csrf_token={csrf}&rating_kind=overall&rating=5")
        )
        .await
        .status(),
        StatusCode::UNPROCESSABLE_ENTITY,
    );
    let saved = call(&app, "POST", &rate, &cookie, ORIGIN, &form).await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    let public = call(&app, "GET", &detail, "", ORIGIN, "").await;
    assert_eq!(public.status(), StatusCode::OK);
    let public_html = html(public).await;
    assert!(public_html.contains("Оценить произведение и перевод"));
    assert!(public_html.contains("5.00") && public_html.contains("Голосов: 1"));
    assert!(!public_html.contains("user_id"));

    let translation = format!("csrf_token={csrf}&rating_kind=translation&rating=2");
    assert_eq!(
        call(&app, "POST", &rate, &cookie, ORIGIN, &translation)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        call(&app, "POST", &rate, &cookie, ORIGIN, &form)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let scores = html(call(&app, "GET", &detail, "", ORIGIN, "").await).await;
    assert!(scores.contains("5.00") && scores.contains("2.00"));
    let work_count: i64 = pg(
        &url,
        &format!(
            "SELECT COUNT(*) FROM user_book_star_ratings
             WHERE user_id={user} AND book_id={book}",
        ),
    )
    .parse()
    .unwrap();
    assert_eq!(
        work_count, 2,
        "repeated vote does not create duplicate rows"
    );
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=true WHERE id={book}"),
    );
    assert_eq!(
        call(&app, "GET", &detail, "", ORIGIN, "").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "GET", &rate, &cookie, ORIGIN, "").await.status(),
        StatusCode::NOT_FOUND
    );
    pool.close().await;
}
