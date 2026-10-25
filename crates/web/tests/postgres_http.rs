use axum::{body::{Body, to_bytes}, http::{Request,StatusCode}};
use tor_books::repositories::PgCatalog;
use tower::ServiceExt;
async fn get(app: axum::Router, path: &str) -> (StatusCode,String) {
    let res = app.oneshot(Request::builder().uri(path).body(Body::empty()).unwrap()).await.unwrap();
    let status = res.status();
    let body = to_bytes(res.into_body(), 131072).await.unwrap();
    (status, String::from_utf8(body.to_vec()).unwrap())
}
#[tokio::test]
#[ignore = "requires isolated PostgreSQL tor_ci"]
async fn pg_live_http_and_private_books() {
    let url=std::env::var("TEST_DATABASE_URL").unwrap();
    let app=tor_web::router_with_postgres(PgCatalog::connect(&url).await.unwrap());
    assert_eq!(get(app.clone(), "/readyz").await.0,StatusCode::OK);
    let (status,json)=get(app.clone(),"/api/v1/books").await;
    assert_eq!(status,StatusCode::OK);
    assert!(json.contains("TOR CI public"));
    assert!(!json.contains("TOR CI hidden"));
    assert!(!json.contains("TOR CI held"));
    let (status,html)=get(app.clone(),"/catalog").await;
    assert_eq!(status,StatusCode::OK);
    assert!(html.contains("TOR CI public"));
    assert!(!html.contains("TOR CI hidden"));
    assert_eq!(get(app.clone(),"/books/1").await.0,StatusCode::OK);
    assert_eq!(get(app.clone(),"/books/2").await.0,StatusCode::NOT_FOUND);
    assert_eq!(get(app.clone(),"/books/3").await.0,StatusCode::NOT_FOUND);
    assert_eq!(get(app,"/api/v1/books?limit=101").await.0,StatusCode::BAD_REQUEST);
}
