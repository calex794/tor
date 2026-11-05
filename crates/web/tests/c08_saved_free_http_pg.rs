//! C-08/BR-060: reader opens exact private saved-free V1, not current V2.
//! Only disposable PostgreSQL tor_ci. No normal-delete production switch.
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
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

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
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout)
        .expect("UTF-8")
        .trim()
        .to_owned()
}

async fn call(app: &Router, method: &str, url: &str, cookie: &str, body: &str) -> Response {
    let req = Request::builder()
        .method(method)
        .uri(url)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, ORIGIN)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn c_08_saved_free_v1_is_private_after_v2_and_pending_deletion() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL");
    assert!(url.ends_with("/tor_ci"), "no permanent database");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let marker = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let token = generate_session_token();
    let outsider_token = generate_session_token();
    let csrf = generate_session_token();
    let password = hash_password("C08SavedReader2026").unwrap();
    let actor = accounts
        .register_with_session(
            &format!("c08-reader-{marker}@example.invalid"),
            "C08 reader",
            &password,
            &session_token_digest(&token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let _outsider = accounts
        .register_with_session(
            &format!("c08-outsider-{marker}@example.invalid"),
            "C08 outsider",
            &password,
            &session_token_digest(&outsider_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,access_level,owner_user_id)
             VALUES('ci:c08:reader:{marker}','ru','Archived <unsafe>','public','basic',{actor})
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let chapter: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level)
             VALUES({book},1,'Title <unsafe>','public','free') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let v1: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({chapter},1,'Pinned ORIGINAL V1') RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!(
            "INSERT INTO chapter_publications(chapter_id,version_id)
             VALUES({chapter},{v1})"
        ),
    );
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let reader_cookie = format!("tor-dev-session={token}; tor-dev-csrf={csrf}");
    let outsider_cookie = format!("tor-dev-session={outsider_token}; tor-dev-csrf={csrf}");
    let read = format!("/me/library/saved/{chapter}");
    let list = format!("/me/library/{book}/saved");

    assert_eq!(
        call(&app, "GET", &read, &reader_cookie, "").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/api/v1/me/library/{book}"),
            &reader_cookie,
            &format!("csrf_token={csrf}")
        )
        .await
        .status(),
        StatusCode::NO_CONTENT
    );
    let saved = call(&app, "GET", &read, &reader_cookie, "").await;
    assert_eq!(saved.status(), StatusCode::OK);
    assert_eq!(saved.headers()[header::CACHE_CONTROL], "no-store");
    assert!(saved.headers().contains_key("content-security-policy"));
    let text = html(saved).await;
    assert!(text.contains("Pinned ORIGINAL V1"));
    assert!(text.contains("Archived &lt;unsafe&gt;"));
    assert!(!text.contains("Archived <unsafe>"));
    let contents = html(call(&app, "GET", &list, &reader_cookie, "").await).await;
    assert!(contents.contains(&format!("href=\"{read}\"")));
    assert!(
        !contents.contains("Pinned ORIGINAL V1"),
        "TOC is metadata only"
    );

    // Private saved-free reader cannot be accessed by another signed-in reader.
    assert_eq!(
        call(&app, "GET", &read, &outsider_cookie, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_ne!(
        call(&app, "GET", &read, "", "").await.status(),
        StatusCode::OK
    );

    let v2: i64 = pg(
        &url,
        &format!(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES({chapter},2,'Current V2 should stay outside private V1 pin')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    pg(
        &url,
        &format!("UPDATE chapter_publications SET version_id={v2} WHERE chapter_id={chapter}"),
    );
    let deletion = format!("/me/books/{book}/settings/delete");
    assert_eq!(
        call(&app, "GET", &deletion, &outsider_cookie, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let first_form = html(call(&app, "GET", &deletion, &reader_cookie, "").await).await;
    assert!(first_form.contains("value=\"start\""));
    assert_eq!(
        call(
            &app,
            "POST",
            &deletion,
            &reader_cookie,
            &format!("csrf_token={csrf}&action=start&expected_revision=1&account_password=wrong")
        )
        .await
        .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &deletion,
            &reader_cookie,
            &format!("csrf_token={csrf}&action=start&expected_revision=1&account_password=C08SavedReader2026")
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let pending_form = html(call(&app, "GET", &deletion, &reader_cookie, "").await).await;
    assert!(pending_form.contains("value=\"complete\""));
    let pending_library = html(call(&app, "GET", "/me/library", &reader_cookie, "").await).await;
    assert!(pending_library.contains("Ожидает удаления"));
    let pending = html(call(&app, "GET", &read, &reader_cookie, "").await).await;
    assert!(pending.contains("Pinned ORIGINAL V1"));
    assert!(!pending.contains("Current V2 should stay outside private V1 pin"));

    // A completed ordinary deletion deindexes public pages but keeps the
    // saved exact private edition and Book tombstone for its original reader.
    assert_eq!(
        call(
            &app, "POST", &deletion, &outsider_cookie,
            &format!("csrf_token={csrf}&action=complete&expected_revision=2&account_password=C08SavedReader2026")
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &deletion,
            &reader_cookie,
            &format!("csrf_token={csrf}&action=complete&expected_revision=2&account_password=C08SavedReader2026")
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    let events = pg(
        &url,
        &format!("SELECT COUNT(*) FROM book_ordinary_deletion_events WHERE book_id={book}"),
    );
    assert_eq!(events, "2");
    let deleted_form = html(call(&app, "GET", &deletion, &reader_cookie, "").await).await;
    assert!(!deleted_form.contains("type=\"password\""));
    assert_eq!(
        call(&app, "GET", &format!("/books/{book}"), "", "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(&app, "GET", &format!("/read/{chapter}"), &reader_cookie, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );
    let tombstone = html(call(&app, "GET", "/me/library", &reader_cookie, "").await).await;
    assert!(tombstone.contains("Удалена"));
    assert!(tombstone.contains(&format!("/me/library/{book}/saved")));
    let still_saved = html(call(&app, "GET", &read, &reader_cookie, "").await).await;
    assert!(still_saved.contains("Pinned ORIGINAL V1"));
    assert!(!still_saved.contains("Current V2 should stay outside private V1 pin"));
    assert_eq!(
        call(&app, "GET", &read, &outsider_cookie, "")
            .await
            .status(),
        StatusCode::NOT_FOUND
    );

    pg(
        &url,
        &format!("UPDATE books SET admin_hold=TRUE WHERE id={book}"),
    );
    assert_eq!(
        call(&app, "GET", &read, &reader_cookie, "").await.status(),
        StatusCode::NOT_FOUND
    );
    pg(
        &url,
        &format!("UPDATE books SET admin_hold=FALSE, access_level='adult' WHERE id={book}"),
    );
    assert_eq!(
        call(&app, "GET", &read, &reader_cookie, "").await.status(),
        StatusCode::NOT_FOUND
    );
    pg(
        &url,
        &format!("UPDATE books SET access_level='basic' WHERE id={book}"),
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/me/library/{book}/remove"),
            &reader_cookie,
            &format!("csrf_token={csrf}")
        )
        .await
        .status(),
        StatusCode::SEE_OTHER
    );
    assert_eq!(
        call(&app, "GET", &read, &reader_cookie, "").await.status(),
        StatusCode::NOT_FOUND
    );

    // Ordinary deletion retains the Book as an audited tombstone.
    // The disposable tor_ci database is discarded after this job, so fixture
    // cleanup must not physically delete records protected by RESTRICT.
    let deletion_sql = format!("DELETE FROM books WHERE id={book}");
    let refused = Command::new("psql")
        .args([
            "-X",
            "-A",
            "-t",
            "-q",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            url.as_str(),
            "--command",
            deletion_sql.as_str(),
        ])
        .output()
        .expect("psql client");
    assert!(
        !refused.status.success()
            && String::from_utf8_lossy(&refused.stderr).contains("book_members_book_id_fkey"),
        "ordinary deletion must not physically erase a Book with protected membership"
    );
    assert_eq!(
        pg(&url, &format!("SELECT COUNT(*) FROM books WHERE id={book}")),
        "1"
    );
    pool.close().await;
}
