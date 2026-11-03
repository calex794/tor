//! A-05: real PostgreSQL HTTP moderation and fail-closed authorization.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    response::Response,
    Router,
};
use std::{num::NonZeroU32, time::{SystemTime, UNIX_EPOCH}};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_contracts::auth::{AccountRole, UserId};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

const ORIGIN: &str = "http://127.0.0.1:3098";

async fn request(app: &Router, method: &str, path: &str, cookie: &str, origin: &str, body: &str) -> Response {
    app.clone()
        .oneshot(
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, origin)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(body.to_owned()))
                .unwrap(),
        )
        .await
        .unwrap()
}

async fn html(response: Response) -> String {
    String::from_utf8(to_bytes(response.into_body(), 256 * 1024).await.unwrap().to_vec()).unwrap()
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 tor_ci"]
async fn moderator_can_hold_and_release_book_without_publication_or_admin_privileges() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let password = hash_password("ModerationBookHoldTestPassword2026!").unwrap();
    let ttl = NonZeroU32::new(3600).unwrap();
    let admin_token = generate_session_token();
    let moderator_token = generate_session_token();
    let outsider_token = generate_session_token();
    let csrf = generate_session_token();
    let admin = accounts.register_with_session(
        &format!("mod-books-admin-{nonce}@example.invalid"),
        "Admin", &password, &session_token_digest(&admin_token), ttl,
    ).await.unwrap();
    let moderator = accounts.register_with_session(
        &format!("mod-books-moderator-{nonce}@example.invalid"),
        "Moderator", &password, &session_token_digest(&moderator_token), ttl,
    ).await.unwrap();
    accounts.register_with_session(
        &format!("mod-books-outsider-{nonce}@example.invalid"),
        "Outsider", &password, &session_token_digest(&outsider_token), ttl,
    ).await.unwrap();
    // The fixture alone bootstraps one administrator, never a user endpoint.
    sqlx::query("INSERT INTO account_roles(user_id,role,is_active) VALUES($1,'administrator',TRUE)")
        .bind(admin).execute(&pool).await.unwrap();
    accounts.change_account_role(
        &session_token_digest(&admin_token),
        UserId::new(moderator).unwrap(),
        AccountRole::Moderator,
        true,
    ).await.unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES($1,'ru','<script>private moderation title</script>','public') RETURNING id",
    )
    .bind(format!("ci:moderation:book:{nonce}"))
    .fetch_one(&pool).await.unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );
    let path = "/moderation/books";
    let action = format!("/moderation/books/{book}/hold");
    let mod_cookie = format!("tor-dev-session={moderator_token}; tor-dev-csrf={csrf}");
    let admin_cookie = format!("tor-dev-session={admin_token}; tor-dev-csrf={csrf}");
    let outsider_cookie = format!("tor-dev-session={outsider_token}; tor-dev-csrf={csrf}");

    assert_eq!(request(&app, "GET", path, "", ORIGIN, "").await.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(request(&app, "GET", path, &outsider_cookie, ORIGIN, "").await.status(), StatusCode::FORBIDDEN);
    assert_eq!(request(&app, "POST", &action, &outsider_cookie, ORIGIN,
        &format!("csrf_token={csrf}&expected_hold=false&new_hold=true")).await.status(), StatusCode::FORBIDDEN);
    let page = request(&app, "GET", path, &mod_cookie, ORIGIN, "").await;
    assert_eq!(page.status(), StatusCode::OK);
    assert_eq!(page.headers()[header::CACHE_CONTROL], "no-store");
    let content = html(page).await;
    assert!(content.contains("&lt;script&gt;private moderation title&lt;/script&gt;"));
    assert!(!content.contains("<script>private moderation title</script>"));
    assert!(!content.contains("body_markdown"));
    assert_eq!(request(&app, "GET", "/moderation/books?before=-1", &mod_cookie, ORIGIN, "")
        .await.status(), StatusCode::BAD_REQUEST);

    let form = format!("csrf_token={csrf}&expected_hold=false&new_hold=true");
    assert_eq!(request(&app, "POST", &action, &mod_cookie, "https://evil.invalid", &form)
        .await.status(), StatusCode::FORBIDDEN);
    assert_eq!(request(&app, "POST", &action, &format!("tor-dev-session={moderator_token}"), ORIGIN, &form)
        .await.status(), StatusCode::FORBIDDEN);
    let applied = request(&app, "POST", &action, &mod_cookie, ORIGIN, &form).await;
    assert_eq!(applied.status(), StatusCode::SEE_OTHER);
    let current: (String, bool) =
        sqlx::query_as("SELECT visibility,admin_hold FROM books WHERE id=$1")
            .bind(book).fetch_one(&pool).await.unwrap();
    assert_eq!(current, ("public".to_owned(), true));
    assert_eq!(request(&app, "GET", &format!("/books/{book}"), "", ORIGIN, "")
        .await.status(), StatusCode::NOT_FOUND);
    assert_eq!(request(&app, "POST", &action, &mod_cookie, ORIGIN, &form)
        .await.status(), StatusCode::CONFLICT);
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM moderation_book_hold_events WHERE book_id=$1",
    )
    .bind(book).fetch_one(&pool).await.unwrap();
    assert_eq!(audit_count, 1, "stale submit must not duplicate audit");

    accounts.change_account_role(
        &session_token_digest(&admin_token),
        UserId::new(moderator).unwrap(),
        AccountRole::Moderator,
        false,
    ).await.unwrap();
    assert_eq!(request(&app, "GET", path, &mod_cookie, ORIGIN, "")
        .await.status(), StatusCode::FORBIDDEN);
    let release = format!("csrf_token={csrf}&expected_hold=true&new_hold=false");
    assert_eq!(request(&app, "POST", &action, &mod_cookie, ORIGIN, &release)
        .await.status(), StatusCode::FORBIDDEN);
    assert_eq!(request(&app, "POST", &action, &admin_cookie, ORIGIN, &release)
        .await.status(), StatusCode::SEE_OTHER);
    let current: (String, bool) =
        sqlx::query_as("SELECT visibility,admin_hold FROM books WHERE id=$1")
            .bind(book).fetch_one(&pool).await.unwrap();
    assert_eq!(current, ("public".to_owned(), false));
    let events: Vec<(i64, bool, bool)> = sqlx::query_as(
        "SELECT actor_user_id,before_hold,after_hold
         FROM moderation_book_hold_events WHERE book_id=$1 ORDER BY id",
    ).bind(book).fetch_all(&pool).await.unwrap();
    assert_eq!(events, vec![(moderator, false, true), (admin, true, false)]);
}
