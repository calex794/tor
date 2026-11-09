//! S049: isolated PostgreSQL 18 and real Axum HTTP for public free-chapter complaints.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::{num::NonZeroU32, process::Command, time::{SystemTime, UNIX_EPOCH}};
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
    let output = Command::new("psql")
        .args(["-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1", "--dbname", url, "--command", sql])
        .output()
        .expect("isolated psql available");
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

async fn request(
    app: &Router,
    method: &str,
    path: &str,
    cookie: &str,
    origin: &str,
    body: &str,
) -> Response {
    let req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::COOKIE, cookie)
        .header(header::ORIGIN, origin)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(body.to_owned()))
        .unwrap();
    app.clone().oneshot(req).await.unwrap()
}

#[tokio::test]
#[ignore = "disposable PostgreSQL 18 tor_ci only"]
async fn s049_free_comment_exact_published_version_live_age_csrf_idempotency() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1:5432"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(catalog.pool());
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let session = generate_session_token();
    let csrf = generate_session_token();
    let user = accounts
        .register_with_session(
            &format!("s049-free-reporter-{stamp}@example.invalid"),
            "Reporter",
            &hash_password("S049FreeChapterVeryStrongPassword").unwrap(),
            &session_token_digest(&session),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(&url, &format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,access_level)
         VALUES('ci:s049:free:{stamp}','ru','S049 free test','public','basic') RETURNING id"
    )).parse().unwrap();
    let chapter: i64 = pg(&url, &format!(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES({book},1,'Free chapter','public','free',0) RETURNING id"
    )).parse().unwrap();
    let version: i64 = pg(&url, &format!(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES({chapter},1,'S049_FREE_CHAPTER_BODY_NOT_REPORT') RETURNING id"
    )).parse().unwrap();
    pg(&url, &format!(
        "INSERT INTO chapter_publications(chapter_id,version_id) VALUES({chapter},{version})"
    ));
    let comment: i64 = pg(&url, &format!(
        "INSERT INTO discussion_comments
         (author_user_id,chapter_id,version_id,body,visibility,command_key,command_hash)
         VALUES({user},{chapter},{version},'E public free comment','visible',
                '{}',decode(repeat('31',32),'hex')) RETURNING id",
        "P".repeat(43)
    )).parse().unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog.clone(), PgReader::new(catalog.pool()), accounts,
        false, ORIGIN.to_owned(),
    );
    let route = format!("/reader/{chapter}/comments/{comment}/report");
    let form_path = format!("{route}?version_id={version}");
    let cookie = format!("tor-dev-session={session}; tor-dev-csrf={csrf}");
    let key = "Z".repeat(43);
    let payload = format!(
        "csrf_token={csrf}&version_id={version}&command_key={key}&reason=spam&details=Offensive+comment"
    );
    assert_eq!(
        request(&app, "GET", &form_path, "", ORIGIN, "").await.status(),
        StatusCode::UNAUTHORIZED,
    );
    let form = request(&app, "GET", &form_path, &cookie, ORIGIN, "").await;
    assert_eq!(form.status(), StatusCode::OK);
    assert_eq!(form.headers()[header::CACHE_CONTROL], "no-store");
    let html = String::from_utf8(
        to_bytes(form.into_body(), 128 * 1024).await.unwrap().to_vec()
    ).unwrap();
    assert!(html.contains("Жалоба на комментарий"));
    assert!(!html.contains("S049_FREE_CHAPTER_BODY_NOT_REPORT"));
    let reader = request(&app, "GET", &format!("/reader/{chapter}"), &cookie, ORIGIN, "").await;
    let reader_html = String::from_utf8(
        to_bytes(reader.into_body(), 256 * 1024).await.unwrap().to_vec()
    ).unwrap();
    assert!(reader_html.contains(&form_path), "free reader must offer a report link");

    assert_eq!(
        request(&app, "POST", &route, &cookie, "https://evil.invalid", &payload).await.status(),
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        request(&app, "POST", &route, &format!("tor-dev-session={session}"), ORIGIN, &payload)
            .await.status(),
        StatusCode::FORBIDDEN,
    );
    let wrong_version = payload.replace(
        &format!("version_id={version}"), "version_id=999999999");
    assert_eq!(
        request(&app, "POST", &route, &cookie, ORIGIN, &wrong_version).await.status(),
        StatusCode::NOT_FOUND,
    );
    let saved = request(&app, "POST", &route, &cookie, ORIGIN, &payload).await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    assert_eq!(
        pg(&url, &format!(
            "SELECT count(*) FROM social_free_comment_reports
             WHERE comment_id={comment} AND chapter_id={chapter} AND version_id={version}
               AND reporter_user_id={user} AND state='pending'"
        )),
        "1"
    );
    assert_eq!(
        request(&app, "POST", &route, &cookie, ORIGIN, &payload).await.status(),
        StatusCode::SEE_OTHER,
    );
    let changed = payload.replace("Offensive+comment", "Different+reason");
    assert_eq!(
        request(&app, "POST", &route, &cookie, ORIGIN, &changed).await.status(),
        StatusCode::CONFLICT,
    );
    let another = payload.replace(&key, &"Q".repeat(43));
    assert_eq!(
        request(&app, "POST", &route, &cookie, ORIGIN, &another).await.status(),
        StatusCode::CONFLICT,
    );
    pg(&url, &format!("UPDATE discussion_comments SET visibility='hidden' WHERE id={comment}"));
    assert_eq!(
        request(&app, "GET", &form_path, &cookie, ORIGIN, "").await.status(),
        StatusCode::NOT_FOUND,
    );
    pg(&url, &format!("UPDATE discussion_comments SET visibility='visible' WHERE id={comment}"));
    pg(&url, &format!("UPDATE books SET access_level='adult' WHERE id={book}"));
    assert_eq!(
        request(&app, "GET", &form_path, &cookie, ORIGIN, "").await.status(),
        StatusCode::NOT_FOUND,
        "adult book is not reportable without live age right"
    );
}
