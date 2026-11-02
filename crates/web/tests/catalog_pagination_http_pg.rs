//! B-18: real catalog HTTP checks without a direct SQL dependency in tor-web.
//! The 47-book, 20+20+7 reverse cursor test lives in tor-search integration.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use tor_books::repositories::PgCatalog;
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
