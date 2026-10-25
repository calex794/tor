use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use tor_books::repositories::PgCatalog;
use tower::ServiceExt;
async fn get(app: axum::Router, path: &str) -> (StatusCode, String) {
    let res = app
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = res.status();
    let body = to_bytes(res.into_body(), 131072).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}
#[tokio::test]
#[ignore = "requires isolated PostgreSQL tor_ci"]
async fn pg_live_http_and_private_books() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let app = tor_web::router_with_postgres(PgCatalog::connect(&url).await.unwrap());
    assert_eq!(get(app.clone(), "/readyz").await.0, StatusCode::OK);
    let (status, json) = get(app.clone(), "/api/v1/books").await;
    assert_eq!(status, StatusCode::OK);
    assert!(json.contains("TOR CI public"));
    assert!(!json.contains("TOR CI hidden"));
    assert!(!json.contains("TOR CI held"));
    let (status, html) = get(app.clone(), "/catalog").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("TOR CI public"));
    assert!(!html.contains("TOR CI hidden"));
    assert_eq!(get(app.clone(), "/books/1").await.0, StatusCode::OK);
    assert_eq!(get(app.clone(), "/books/2").await.0, StatusCode::NOT_FOUND);
    assert_eq!(get(app.clone(), "/books/3").await.0, StatusCode::NOT_FOUND);
    assert_eq!(
        get(app, "/api/v1/books?limit=101").await.0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL and disposable tor_ci test records"]
async fn public_reading_http_renders_safe_markdown_and_blocks_paid_text() {
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    let catalog = PgCatalog::connect(&url).await.expect("tor_app");
    let reader = tor_reading::repositories::PgReader::connect(&url)
        .await
        .expect("tor_app");
    let app = tor_web::router_with_postgres_reading(catalog, reader);
    let (status, book) = get(app.clone(), "/books/1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(book.contains("Глава 1. Начало истории"));
    assert!(book.contains("href=\"/reader/101\""));
    assert!(book.contains("Платная · пока недоступна"));
    assert!(!book.contains("PRIVATE-PAID-MARKER"));
    assert!(!book.contains("Секретный черновик"));
    assert!(!book.contains("Запланированная глава"));
    let (status, html) = get(app.clone(), "/reader/101").await;
    assert_eq!(status, StatusCode::OK);
    assert!(html.contains("<strong>"));
    assert!(html.contains("&lt;script&gt;"));
    assert!(!html.contains("<script>"));
    assert!(html.contains("href=\"/reader/108\""));
    assert!(!html.contains("PRIVATE-PAID-MARKER"));
    for denied in [102, 103, 104, 105, 106, 107] {
        assert_eq!(
            get(app.clone(), &format!("/reader/{denied}")).await.0,
            StatusCode::NOT_FOUND
        );
    }
}
