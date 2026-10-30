//! NEwBOO book stats: browser routes are private and use real Book membership.
#![forbid(unsafe_code)]
use axum::{
    body::{to_bytes, Body},
    http::{header, Request, StatusCode},
    Router,
};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, hash_password, session_token_digest},
};
use tor_books::repositories::PgCatalog;
use tor_reading::repositories::PgReader;
use tower::ServiceExt;

fn pg(url: &str, statement: &str) -> String {
    let result = std::process::Command::new("psql")
        .args(["-X", "-A", "-t", "-q", "-v", "ON_ERROR_STOP=1", "--dbname", url, "--command", statement])
        .output().expect("psql");
    assert!(result.status.success(), "{}", String::from_utf8_lossy(&result.stderr));
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}

async fn get(app: &Router, path: &str, session: Option<&str>) -> axum::response::Response {
    let mut request = Request::builder().uri(path);
    if let Some(token) = session {
        request = request.header(header::COOKIE, format!("tor-dev-session={token}"));
    }
    app.clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap()
}
async fn body(response: axum::response::Response) -> String {
    String::from_utf8(to_bytes(response.into_body(), 128 * 1024).await.unwrap().to_vec()).unwrap()
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL 18 tor_ci"]
async fn book_stats_page_csv_owner_foreign_guest_and_csv_formulas() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let owner_token = generate_session_token();
    let foreign_token = generate_session_token();
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let owner = accounts
        .register_with_session(
            &format!("stats-owner-{unique}@example.invalid"),
            "Stats owner",
            &hash_password("StatsOwnerTestOnly2026").unwrap(),
            &session_token_digest(&owner_token),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    accounts
        .register_with_session(
            &format!("stats-other-{unique}@example.invalid"),
            "Stats other",
            &hash_password("StatsOtherTestOnly2026").unwrap(),
            &session_token_digest(&foreign_token),
            std::num::NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    let book: i64 = pg(&url, &format!(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id) \
         VALUES('ci:stats-owner:{unique}','ru','=2+3 Stats','public',{owner}) RETURNING id"
    )).parse().unwrap();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        "http://127.0.0.1:3098".to_owned(),
    );
    let page = format!("/books/{book}/stats");
    let csv_path = format!("{page}/export.csv");

    let guest = get(&app, &page, None).await;
    assert_eq!(guest.status(), StatusCode::SEE_OTHER);
    assert_eq!(guest.headers()[header::LOCATION], "/login");
    assert_eq!(
        get(&app, &page, Some(&foreign_token)).await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get(&app, &csv_path, Some(&foreign_token)).await.status(),
        StatusCode::NOT_FOUND
    );
    let ok = get(&app, &page, Some(&owner_token)).await;
    assert_eq!(ok.status(), StatusCode::OK);
    assert_eq!(ok.headers()[header::CACHE_CONTROL], "no-store");
    let html = body(ok).await;
    assert!(html.contains("Статистика книги"));
    assert!(html.contains("=2+3 Stats"));
    let csv = get(&app, &csv_path, Some(&owner_token)).await;
    assert_eq!(csv.status(), StatusCode::OK);
    assert_eq!(csv.headers()[header::CACHE_CONTROL], "no-store");
    let data = body(csv).await;
    assert!(data.contains("\"'=2+3 Stats\""), "formula must not execute");
    assert!(data.contains("book,0,"));
    assert_eq!(
        get(&app, &format!("{page}?after=oops"), Some(&owner_token)).await.status(),
        StatusCode::BAD_REQUEST
    );
    pool.close().await;
}
