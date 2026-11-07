//! B-18: real catalog HTTP checks without a direct SQL dependency in tor-web.
//! The 47-book, 20+20+7 reverse cursor test lives in tor-search integration.
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
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

async fn get(app: axum::Router, path: &str) -> (StatusCode, String) {
    let response = app
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 131072).await.unwrap();
    (status, String::from_utf8(bytes.to_vec()).unwrap())
}

fn cards(html: &str) -> usize {
    html.matches("class=\"book-mini-card book-mini-card--compact\"")
        .count()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci with canonical public fixtures"]
async fn b_18_catalog_http_is_bounded_and_rejects_invalid_page_parameters() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PostgreSQL URL");
    assert!(url.ends_with("/tor_ci"), "never use production");
    let app = tor_web::router_with_postgres(PgCatalog::connect(&url).await.unwrap());

    let (status, first) = get(app.clone(), "/catalog?sort=oldest").await;
    assert_eq!(status, StatusCode::OK);
    assert!(first.contains("Страница 1"));
    assert!(cards(&first) <= 20, "never send all books to a browser");
    assert!(!first.contains("TOR CI hidden"));
    assert!(!first.contains("TOR CI held"));

    let (status, page_two) = get(app.clone(), "/catalog?after=1&page=2&sort=oldest").await;
    assert_eq!(status, StatusCode::OK);
    assert!(page_two.contains("Страница 2"));
    assert!(cards(&page_two) <= 20);
    assert!(!page_two.contains("TOR CI hidden"));

    for invalid in [
        "/catalog?page=2",
        "/catalog?before=wrong&page=1",
        "/catalog?after=1&before=2&page=2",
        "/catalog?after=00&page=2",
        "/catalog?after=1&page=1",
        "/catalog?page=0",
    ] {
        assert_eq!(
            get(app.clone(), invalid).await.0,
            StatusCode::BAD_REQUEST,
            "{invalid}"
        );
    }
}

fn sql(url: &str, query: &str) -> String {
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
            query,
        ])
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

async fn get_as(
    app: &axum::Router,
    uri: &str,
    cookie: &str,
) -> (StatusCode, String, Option<String>) {
    let request = Request::builder()
        .uri(uri)
        .header(header::COOKIE, cookie)
        .body(Body::empty())
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let cache = response
        .headers()
        .get(header::CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let body = to_bytes(response.into_body(), 131_072).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap(), cache)
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s020_catalog_http_rechecks_session_and_age_without_seo_leak() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated PostgreSQL");
    assert!(url.ends_with("/tor_ci"));
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let term = format!("S020HTTPAge{stamp}");
    let session = generate_session_token();
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let accounts = PgAccounts::from_pool(pool.clone());
    let actor = accounts
        .register_with_session(
            &format!("s020-catalog-http-{stamp}@example.invalid"),
            "S020 catalog reader",
            &hash_password("DisposableCatalogTestPassword2026").unwrap(),
            &session_token_digest(&session),
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool),
        accounts,
        false,
        "http://127.0.0.1:3098".to_owned(),
    );
    for (name, level, visibility, lifecycle) in [
        ("basic", "'basic'", "active", "in_progress"),
        ("adult", "'adult'", "active", "in_progress"),
        ("pro", "'pro_adult'", "active", "in_progress"),
        ("unknown", "NULL", "active", "in_progress"),
        ("pending", "'basic'", "active", "pending_deletion"),
        ("hidden", "'basic'", "hidden", "in_progress"),
    ] {
        sql(&url, &format!(
            "INSERT INTO books(source_identity,target_language,title,visibility,visibility_status,lifecycle_status,access_level)
             VALUES('ci:s020:http:{stamp}:{name}','ru','{term} {name}','public','{visibility}','{lifecycle}',{level})"
        ));
    }
    let path = format!("/catalog?q={term}&sort=oldest");
    let cookie = format!("tor-dev-session={session}");
    let check = |html: &str, expected: &[&str]| {
        for name in ["basic", "adult", "pro", "unknown", "pending", "hidden"] {
            assert_eq!(
                html.contains(&format!("{term} {name}")),
                expected.contains(&name),
                "unexpected {name} in catalogue"
            );
        }
    };
    let (status, anonymous, _) = get_as(&app, &path, "").await;
    assert_eq!(status, StatusCode::OK);
    check(&anonymous, &["basic"]);
    let (status, basic, private) = get_as(&app, &path, &cookie).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(private.as_deref(), Some("no-store"));
    check(&basic, &["basic"]);

    sql(
        &url,
        &format!("UPDATE users SET adult_enabled=TRUE WHERE id={actor}"),
    );
    check(&get_as(&app, &path, &cookie).await.1, &["basic", "adult"]);
    sql(
        &url,
        &format!(
            "INSERT INTO user_level_grants(user_id,level,source,is_active)
         VALUES({actor},'pro_adult','moderation',TRUE)"
        ),
    );
    check(
        &get_as(&app, &path, &cookie).await.1,
        &["basic", "adult", "pro"],
    );
    check(&get_as(&app, &path, "").await.1, &["basic"]);
    check(
        &get_as(&app, &format!("/api/v1/search/books?q={term}"), &cookie)
            .await
            .1,
        &["basic"],
    );

    sql(
        &url,
        &format!("UPDATE users SET adult_enabled=FALSE WHERE id={actor}"),
    );
    check(&get_as(&app, &path, &cookie).await.1, &["basic", "pro"]);
    sql(
        &url,
        &format!("UPDATE user_level_grants SET is_active=FALSE WHERE user_id={actor}"),
    );
    check(&get_as(&app, &path, &cookie).await.1, &["basic"]);
    sql(
        &url,
        &format!("UPDATE sessions SET revoked_at=now() WHERE user_id={actor}"),
    );
    check(&get_as(&app, &path, &cookie).await.1, &["basic"]);
}

#[path = "catalog_pagination_http_pg/f04_rising.rs"]
mod f04_rising;
