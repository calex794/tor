//! S020 P0: draft HTTP age settings accept only live scoped, monotonic Book tiers.
//! Mutations are off by default; this fixture uses disposable PostgreSQL tor_ci.
use axum::{
    body::Body,
    http::{header, Request, StatusCode},
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

fn pg(url: &str, query: &str) -> String {
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:5432/"));
    let result = Command::new("psql")
        .args(["-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1", "--dbname", url, "--command", query])
        .output()
        .expect("postgres client");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

async fn get(app: &Router, path: &str, cookie: &str) -> StatusCode {
    let reply = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(path)
                .header(header::COOKIE, cookie)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    reply.status()
}

async fn submit(
    app: &Router,
    path: &str,
    cookie: &str,
    origin: &str,
    csrf: &str,
    expected: &str,
    requested: &str,
) -> StatusCode {
    let encoded = format!(
        "csrf_token={csrf}&expected_level={expected}&requested_level={requested}"
    );
    let reply = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(path)
                .header(header::COOKIE, cookie)
                .header(header::ORIGIN, origin)
                .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
                .body(Body::from(encoded))
                .unwrap(),
        )
        .await
        .unwrap();
    reply.status()
}

#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL /tor_ci"]
async fn s020_age_editor_is_off_by_default_and_monotonic_under_live_http() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG");
    assert!(url.ends_with("/tor_ci") && url.contains("@127.0.0.1:5432/"));
    let marker = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let owner_token = generate_session_token();
    let outsider_token = generate_session_token();
    let csrf = generate_session_token();
    let password = hash_password("S020AgeEditorCI2026!").unwrap();
    let owner = accounts
        .register_with_session(
            &format!("s020-age-owner-{marker}@example.invalid"),
            "S020 age owner",
            &password,
            &session_token_digest(&owner_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    accounts
        .register_with_session(
            &format!("s020-age-stranger-{marker}@example.invalid"),
            "S020 age stranger",
            &password,
            &session_token_digest(&outsider_token),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(
        &url,
        &format!(
            "INSERT INTO books(source_identity,target_language,title,owner_user_id,visibility,access_level)
             VALUES('ci:s020:age:book:{marker}','ru','S020 guarded Book', {owner}, 'public', 'basic')
             RETURNING id"
        ),
    )
    .parse()
    .unwrap();
    let path = format!("/me/books/{book}/settings/access-level");
    let public = format!("/books/{book}");
    let owner_cookie = format!("tor-dev-session={owner_token}; tor-dev-csrf={csrf}");
    let stranger_cookie = format!("tor-dev-session={outsider_token}; tor-dev-csrf={csrf}");

    std::env::remove_var("TOR_S020_ACCESS_EDITOR_ENABLED");
    let disabled = tor_web::router_with_postgres_reading_auth(
        catalog.clone(),
        PgReader::new(pool.clone()),
        accounts.clone(),
        false,
        ORIGIN.to_owned(),
    );
    assert_eq!(get(&disabled, &path, &owner_cookie).await, StatusCode::NOT_FOUND);
    assert_eq!(
        submit(&disabled, &path, &owner_cookie, ORIGIN, &csrf, "basic", "adult").await,
        StatusCode::NOT_FOUND,
    );
    assert_eq!(
        pg(&url, &format!("SELECT access_level FROM books WHERE id={book}")),
        "basic",
    );

    // Tests opt into the route in this disposable process only; no production
    // deployment or persistent DB migration is performed.
    std::env::set_var("TOR_S020_ACCESS_EDITOR_ENABLED", "1");
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    std::env::remove_var("TOR_S020_ACCESS_EDITOR_ENABLED");

    assert_eq!(get(&app, &path, "").await, StatusCode::SEE_OTHER);
    assert_eq!(get(&app, &path, &owner_cookie).await, StatusCode::OK);
    assert_eq!(get(&app, &path, &stranger_cookie).await, StatusCode::NOT_FOUND);
    assert_eq!(get(&app, &public, "").await, StatusCode::OK);
    assert_eq!(
        submit(&app, &path, &stranger_cookie, ORIGIN, &csrf, "basic", "adult").await,
        StatusCode::NOT_FOUND,
    );
    assert_eq!(
        submit(&app, &path, &owner_cookie, "https://wrong.invalid", &csrf, "basic", "adult").await,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        submit(&app, &path, &owner_cookie, ORIGIN, "forged", "basic", "adult").await,
        StatusCode::FORBIDDEN,
    );
    assert_eq!(
        submit(&app, &path, &owner_cookie, ORIGIN, &csrf, "basic", "adult").await,
        StatusCode::SEE_OTHER,
    );
    assert_eq!(
        pg(&url, &format!("SELECT access_level FROM books WHERE id={book}")),
        "adult",
    );
    assert_eq!(get(&app, &public, "").await, StatusCode::NOT_FOUND);
    assert_eq!(
        submit(&app, &path, &owner_cookie, ORIGIN, &csrf, "basic", "pro_adult").await,
        StatusCode::CONFLICT,
    );
    assert_eq!(
        submit(&app, &path, &owner_cookie, ORIGIN, &csrf, "adult", "basic").await,
        StatusCode::NOT_FOUND,
    );
    assert_eq!(
        submit(&app, &path, &owner_cookie, ORIGIN, &csrf, "adult", "pro_adult").await,
        StatusCode::NOT_FOUND,
    );
    pg(
        &url,
        &format!(
            "INSERT INTO user_level_grants(user_id,level,source,is_active)
             VALUES({owner},'pro_adult','moderation',TRUE)"
        ),
    );
    assert_eq!(
        submit(&app, &path, &owner_cookie, ORIGIN, &csrf, "adult", "pro_adult").await,
        StatusCode::SEE_OTHER,
    );
    assert_eq!(
        pg(&url, &format!("SELECT access_level FROM books WHERE id={book}")),
        "pro_adult",
    );
    // Revocation closes reader access; the signed-in Book settings permission
    // is independent of reader age eligibility.
    pg(
        &url,
        &format!("UPDATE user_level_grants SET is_active=FALSE WHERE user_id={owner}"),
    );
    assert_eq!(get(&app, &public, &owner_cookie).await, StatusCode::NOT_FOUND);
    assert_eq!(get(&app, &path, &owner_cookie).await, StatusCode::OK);

    // Deletion cannot be edited through the ordinary settings route.
    pg(
        &url,
        &format!(
            "UPDATE books SET lifecycle_status='pending_deletion' WHERE id={book}"
        ),
    );
    assert_eq!(get(&app, &path, &owner_cookie).await, StatusCode::NOT_FOUND);
    assert_eq!(
        submit(&app, &path, &owner_cookie, ORIGIN, &csrf, "pro_adult", "pro_adult").await,
        StatusCode::NOT_FOUND,
    );
    pool.close().await;
}
