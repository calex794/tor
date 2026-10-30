//! NEwBOO one-time chapter thanks via actual Axum + disposable PostgreSQL.
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

fn pg(url: &str, sql: &str) -> String {
    assert!(url.ends_with("/tor_ci"));
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
        .expect("psql available in PG integration workflow");
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
async fn body(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 128 * 1024)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn c_thanks_http_csrf_session_repeat_hidden_and_reader_link() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
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
            &format!("thanks-http-{nonce}@example.invalid"),
            "Читатель",
            &hash_password("ThanksBrowserPassword2026").unwrap(),
            &session_token_digest(&session),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility)
             VALUES('ci:thanks-http:{nonce}','ru','Книга','public') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},1,'Глава «Спасибо»','public','free',0) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({chapter},1,'Содержимое главы') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
        ),
    );
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let path = format!("/reader/{chapter}/thanks");
    let cookie = format!("tor-dev-session={session}; tor-dev-csrf={csrf}");
    let form = format!("csrf_token={csrf}");
    assert_eq!(
        call(&app, "GET", &path, "", ORIGIN, "").await.status(),
        StatusCode::SEE_OTHER
    );
    let page = call(&app, "GET", &path, &cookie, ORIGIN, "").await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()[header::CACHE_CONTROL], "no-store");
    let markup = body(page).await;
    assert!(markup.contains("Сказать «Спасибо»"));
    assert!(markup.contains("Благодарностей: 0"));
    let reader = body(call(&app, "GET", &format!("/reader/{chapter}"), "", ORIGIN, "").await).await;
    assert!(
        reader.contains(&path),
        "public reader links to original thanks action"
    );
    assert_eq!(
        call(&app, "POST", &path, &cookie, "http://evil.invalid", &form)
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&app, "POST", &path, &cookie, ORIGIN, "csrf_token=forged")
            .await
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &path,
            &format!("tor-dev-csrf={csrf}"),
            ORIGIN,
            &form
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    let added = call(&app, "POST", &path, &cookie, ORIGIN, &form).await;
    assert_eq!(added.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        added.headers()[header::LOCATION],
        format!("{path}?done=added")
    );
    let repeated = call(&app, "POST", &path, &cookie, ORIGIN, &form).await;
    assert_eq!(repeated.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        repeated.headers()[header::LOCATION],
        format!("{path}?done=repeat")
    );
    let count: i64 = pg(
        &url,
        &format!(
            "SELECT COUNT(*) FROM chapter_thanks WHERE chapter_id={chapter} AND user_id={user}"
        ),
    )
    .parse()
    .unwrap();
    assert_eq!(count, 1);
    assert!(body(call(&app, "GET", &path, &cookie, ORIGIN, "").await)
        .await
        .contains("Благодарностей: 1"));
    pg(
        &url,
        &format!("UPDATE chapters SET admin_hold=true WHERE id={chapter}"),
    );
    assert_eq!(
        call(&app, "POST", &path, &cookie, ORIGIN, &form)
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "GET", &path, &cookie, ORIGIN, "").await.status(),
        StatusCode::NOT_FOUND
    );
    pool.close().await;
}
