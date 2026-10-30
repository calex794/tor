//! C-15: authenticated demand and owner aggregate via real isolated PostgreSQL.
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

/// Keep SQLx outside tor-web. These are test-only fixtures using the test role.
fn pg(url: &str, sql: &str) -> String {
    assert!(url.ends_with("/tor_ci"), "production DB forbidden");
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
        .expect("psql client");
    assert!(
        result.status.success(),
        "fixture SQL failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

async fn call(
    app: &Router,
    method: &str,
    path: &str,
    cookies: &str,
    origin: &str,
    body: &str,
) -> Response {
    let request = Request::builder()
        .uri(path)
        .method(method)
        .header(header::COOKIE, cookies)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .expect("HTTP request");
    app.clone().oneshot(request).await.expect("HTTP response")
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
    let pool = catalog.pool().clone();
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

    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
             VALUES('ci:c15:http:{nonce}','ru','Книга для читателей','public',{owner}) RETURNING id"
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
             VALUES({chapter},1,'Первая глава') RETURNING id"
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

    pg(
        &url,
        &format!(
            "INSERT INTO reader_chapter_completions
             (user_id,book_id,chapter_id,first_version_id,source_event_id)
             VALUES({reader},{book},{chapter},{version},gen_random_uuid())"
        ),
    );

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
    let count: i64 = pg(
        &url,
        &format!(
            "SELECT count(*) FROM reader_continuation_demand
             WHERE user_id={reader} AND book_id={book} AND frontier_chapter_id={chapter}"
        ),
    )
    .parse()
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

    // C-15/B-06: close the minimal on-site loop via the actual author Save,
    // rather than only fixture SQL. External email/push is out of scope.
    let follow_path = format!("/me/books/{book}/follow");
    assert_eq!(
        call(&app, "POST", &follow_path, &reader_cookie, ORIGIN, &form)
            .await
            .status(),
        StatusCode::SEE_OTHER
    );
    let save_path = format!("/me/books/{book}/chapters");
    let save_body = format!(
        "number=2&title=Demand+fulfilled&body=C15%20reader%20return&access=free&price_amount=0&command_key={}&csrf_token={csrf}",
        generate_session_token()
    );
    let denied = call(&app, "POST", &save_path, &reader_cookie, ORIGIN, &save_body)
        .await
        .status();
    assert!(
        matches!(denied, StatusCode::FORBIDDEN | StatusCode::NOT_FOUND),
        "a reader must not publish the requested continuation: {denied}"
    );
    assert_eq!(
        pg(
            &url,
            &format!("SELECT count(*) FROM chapters WHERE book_id={book} AND chapter_no=2")
        ),
        "0"
    );
    for _ in 0..2 {
        assert_eq!(
            call(&app, "POST", &save_path, &owner_cookie, ORIGIN, &save_body)
                .await
                .status(),
            StatusCode::SEE_OTHER,
            "author Save must publish once and replay idempotently"
        );
    }
    let released: i64 = pg(
        &url,
        &format!("SELECT id FROM chapters WHERE book_id={book} AND chapter_no=2"),
    )
    .parse()
    .unwrap();
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM chapter_first_release_events WHERE chapter_id={released}"
            )
        ),
        "1",
        "one author Save must create exactly one reader-visible first release"
    );
    let follower_updates =
        html(call(&app, "GET", "/me/library", &reader_cookie, ORIGIN, "").await).await;
    assert!(follower_updates.contains("Demand fulfilled"));
    assert!(follower_updates.contains(&format!("/me/updates/{released}/read-state")));
    let returned = html(
        call(
            &app,
            "GET",
            &format!("/reader/{released}"),
            &reader_cookie,
            ORIGIN,
            "",
        )
        .await,
    )
    .await;
    assert!(returned.contains("C15 reader return"));
    assert_eq!(
        pg(
            &url,
            &format!(
                "SELECT count(*) FROM reader_chapter_completions \
                 WHERE user_id={reader} AND chapter_id={released}"
            )
        ),
        "0",
        "opening a new chapter is not itself a meaningful completion"
    );
    let owner_fulfilled =
        html(call(&app, "GET", &owner_path, &owner_cookie, ORIGIN, "").await).await;
    assert!(owner_fulfilled
        .contains("Запросов после последней опубликованной главы: <strong>0</strong>"));
    // A separate synthetic paid release remains as the old non-leak regression.

    let next: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES({book},3,'Продолжение','public','paid',150) RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let paid_version: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({next},1,'PAID-CONTENT-SECRET') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id)
             VALUES({next},{paid_version})"
        ),
    );
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
    // The isolated tor_ci database is disposed after CI. Book ownership rows
    // deliberately have RESTRICT integrity; avoid false failures from a direct
    // book DELETE that bypasses the application ownership lifecycle.
    pool.close().await;
}
