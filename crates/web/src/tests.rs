use super::*;
use axum::{
    body::{to_bytes, Body},
    http::Request,
};
use tor_books::repositories::InMemoryCatalog;
use tower::ServiceExt;
async fn get(path: &str) -> (StatusCode, String) {
    let app = router(Arc::new(InMemoryCatalog::demo()));
    let response = app
        .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}
#[tokio::test]
async fn demo_serves_real_html_routes() {
    for path in [
        "/",
        "/catalog",
        "/books/1",
        "/login",
        "/register",
        "/healthz",
        "/readyz",
        "/assets/site.css",
    ] {
        assert_eq!(get(path).await.0, StatusCode::OK, "{path}");
    }
}

#[tokio::test]
async fn seo_03_book_urls_have_one_public_spelling() {
    for path in ["/books/1", "/books/1?limit=1"] {
        assert_eq!(get(path).await.0, StatusCode::OK, "{path}");
    }
    for path in ["/books/0", "/books/01", "/books/0001"] {
        assert_eq!(get(path).await.0, StatusCode::NOT_FOUND, "{path}");
    }
    for path in [
        "/books/+1",
        "/books/%31",
        "/books/1/",
        "/books/18446744073709551616",
        "/books/%D0%BA",
    ] {
        let (status, html) = get(path).await;
        assert_ne!(status, StatusCode::OK, "{path}");
        assert!(!html.contains("book-hero-card"), "{path}");
    }
    assert_eq!(get("/books/3").await.0, StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn hidden_book_never_reaches_public_pages() {
    for path in ["/", "/catalog", "/api/v1/books"] {
        let (_, body) = get(path).await;
        assert!(body.contains("Первое знакомство"));
        assert!(!body.contains("Закрытый черновик"));
    }
    assert_eq!(get("/books/3").await.0, StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn demo_must_not_advertise_unimplemented_reader() {
    assert_eq!(get("/reader/1").await.0, StatusCode::NOT_FOUND);
    let app = router(Arc::new(InMemoryCatalog::demo()));
    let reply = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/reader/chapters/1/delivery-grants")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(reply.status(), StatusCode::NOT_FOUND);
}
#[tokio::test]
async fn operator_summary_must_reject_site_account_cookie_without_staff_secret() {
    assert_eq!(get("/ops/overview").await.0, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn operator_diagnostics_never_open_without_independent_credentials() {
    for path in [
        "/ops/traces",
        "/ops/traces?id=00000000000000000000000000000010",
        "/ops/traces?id=%3Cscript%3E",
    ] {
        // Even when the query is invalid, access control runs first.
        assert_eq!(get(path).await.0, StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn no_user_or_finance_write_routes() {
    assert_eq!(
        get("/api/v1/books?limit=10000").await.0,
        StatusCode::BAD_REQUEST
    );
    let app = router(Arc::new(InMemoryCatalog::demo()));
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/register")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
}
