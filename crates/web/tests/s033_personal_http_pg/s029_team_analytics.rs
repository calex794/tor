//! S029 Team analytics HTML and CSV share the same Team/book filters.
use super::{body, cookie, csrf_from_page, send, signup, ORIGIN};
use axum::http::{header, StatusCode};
use std::time::{SystemTime, UNIX_EPOCH};
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_books::{
    repositories::{postgres::PgAuthoring, PgCatalog},
    services::authoring::new_book,
};
use tor_reading::repositories::PgReader;
use tor_accounts::services::security::generate_session_token;

#[tokio::test]
#[ignore = "disposable migrated PostgreSQL tor_ci"]
async fn s029_analytics_csv_matches_ui_book_period_scope_without_foreign_book_data() {
    let url = std::env::var("TEST_DATABASE_URL").expect("PG test database");
    assert!(url.ends_with("/tor_ci"), "never write to permanent PG");
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool();
    let accounts = PgAccounts::from_pool(pool.clone());
    let lookup = accounts.clone();
    let app = tor_web::router_with_postgres_reading_auth(
        catalog,
        PgReader::new(pool.clone()),
        accounts,
        false,
        ORIGIN.to_owned(),
    );
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let owner = signup(&app, &format!("s029-owner-{stamp}@example.invalid")).await;
    let foreign = signup(&app, &format!("s029-foreign-{stamp}@example.invalid")).await;
    let owner_id = lookup.active_session_user(&session_token_digest(owner.split_once('=').unwrap().1)).await.unwrap().unwrap();
    let foreign_id = lookup.active_session_user(&session_token_digest(foreign.split_once('=').unwrap().1)).await.unwrap().unwrap();
    let team_page = send(&app, "GET", "/me/teams", &owner, "").await;
    let token = csrf_from_page(&body(team_page).await);
    let cookie_csrf = cookie(&send(&app, "GET", "/me/teams", &owner, "").await, "tor-dev-csrf=");
    let owner_cookies = format!("{owner}; {cookie_csrf}");
    let created = send(&app, "POST", "/me/teams", &owner_cookies, &format!("name=Stats{stamp}&csrf_token={token}")).await;
    assert_eq!(created.status(), StatusCode::SEE_OTHER);
    let team = created.headers()[header::LOCATION].to_str().unwrap().to_owned();
    let authoring = PgAuthoring::new(pool.clone());
    let mine = new_book(&format!("s029:owner:{stamp}"), "ru", &format!("A Book {stamp}"), &generate_session_token()).unwrap();
    let own_book_id = authoring.create_owned_book(owner_id, &mine).await.unwrap();
    let theirs = new_book(&format!("s029:foreign:{stamp}"), "ru", &format!("Z Book {stamp}"), &generate_session_token()).unwrap();
    let other_book_id = authoring.create_owned_book(foreign_id, &theirs).await.unwrap();
    let linked = send(&app, "POST", &format!("{team}/books"), &owner_cookies, &format!("book_id={own_book_id}&csrf_token={token}")).await;
    assert_eq!(linked.status(), StatusCode::SEE_OTHER);
    let scoped = format!("{team}/analytics?period=7d&book_id={own_book_id}");
    let html = body(send(&app, "GET", &scoped, &owner, "").await).await;
    assert!(html.contains(&format!("A Book {stamp}")));
    assert!(!html.contains(&format!("Z Book {stamp}")));
    assert!(html.contains("7 дней"));
    let csv_path = format!("{team}/analytics.csv?period=7d&book_id={own_book_id}");
    let csv_response = send(&app, "GET", &csv_path, &owner, "").await;
    assert_eq!(csv_response.status(), StatusCode::OK);
    assert_eq!(csv_response.headers()[header::CONTENT_TYPE], "text/csv; charset=utf-8");
    assert_eq!(csv_response.headers()[header::CACHE_CONTROL], "no-store");
    let csv = body(csv_response).await;
    assert!(csv.contains(&format!("A Book {stamp}")));
    assert!(csv.contains(&format!(";{own_book_id};")));
    assert!(!csv.contains(&format!("Z Book {stamp}")));
    assert!(!csv.contains(&format!(";{other_book_id};")));
    assert_eq!(csv.lines().count(), 3, "header, total, one linked book");
    for route in [&scoped, &csv_path] {
        assert_eq!(send(&app, "GET", route, &foreign, "").await.status(), StatusCode::NOT_FOUND);
    }
    for route in [
        format!("{team}/analytics?period=30d&book_id={other_book_id}"),
        format!("{team}/analytics.csv?period=30d&book_id={other_book_id}"),
    ] {
        assert_eq!(send(&app, "GET", &route, &owner, "").await.status(), StatusCode::NOT_FOUND);
    }
    assert_eq!(
        send(&app, "GET", &format!("{team}/analytics?period=200d"), &owner, "").await.status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(send(&app, "GET", &format!("{team}/analytics.csv"), "", "").await.status(), StatusCode::SEE_OTHER);
}
