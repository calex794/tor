//! B-18: the real /catalog HTTP page never sends the full catalog to a browser.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use sqlx::PgPool;
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

fn navigation_href(html: &str, direction: &str) -> Option<String> {
    let marker = format!("rel=\"{direction}\" href=\"");
    let url = html.split(&marker).nth(1)?.split('"').next()?;
    Some(url.replace("&amp;", "&"))
}

fn cards(html: &str) -> usize {
    html.matches("class=\"book-mini-card book-mini-card--compact\"")
        .count()
}

#[tokio::test]
#[ignore = "disposable PostgreSQL 18 tor_ci only"]
async fn b_18_catalog_http_47_books_are_split_into_three_reversible_pages() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL URL");
    assert!(url.ends_with("/tor_ci"), "no production database");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce: i64 =
        sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM clock_timestamp()) * 1000000)::bigint")
            .fetch_one(&pool)
            .await
            .unwrap();
    let term = format!("WebPager{nonce}");
    sqlx::query(
        "INSERT INTO books(source_identity,target_language,title,author_name,
                 genres,visibility,admin_hold,is_deleted)
         SELECT $1 || n::text,'ru',$2 || ' ' || lpad(n::text,3,'0'),
                'B18 Author',ARRAY['b18-genre'],
                CASE WHEN n<=47 THEN 'public' ELSE 'draft' END,n=49,n=50
         FROM generate_series(1,50) AS n",
    )
    .bind(format!("ci:b18:web:{nonce}:"))
    .bind(&term)
    .execute(&pool)
    .await
    .unwrap();
    let app = tor_web::router_with_postgres(PgCatalog::connect(&url).await.unwrap());
    let start = format!("/catalog?q={term}&genre=b18-genre&language=ru&sort=title");
    let (status, first) = get(app.clone(), &start).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cards(&first), 20, "never send 50 or 100k cards to browser");
    assert!(first.contains("Страница 1"));
    assert!(first.contains("Найдено книг: 47"));
    assert!(navigation_href(&first, "prev").is_none());

    let second_url = navigation_href(&first, "next").unwrap();
    assert!(second_url.contains("page=2"));
    assert!(second_url.contains("genre=b18-genre"));
    assert!(second_url.contains("sort=title"));
    let (status, second) = get(app.clone(), &second_url).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cards(&second), 20);
    assert!(second.contains("Страница 2"));
    assert!(navigation_href(&second, "prev").is_some());

    let third_url = navigation_href(&second, "next").unwrap();
    assert!(third_url.contains("page=3"));
    let (status, third) = get(app.clone(), &third_url).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cards(&third), 7);
    assert!(third.contains("Страница 3"));
    assert!(navigation_href(&third, "next").is_none());

    let (status, back_two) =
        get(app.clone(), &navigation_href(&third, "prev").unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cards(&back_two), 20);
    assert!(back_two.contains("Страница 2"));
    let (status, back_one) =
        get(app.clone(), &navigation_href(&back_two, "prev").unwrap()).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cards(&back_one), 20);
    assert!(back_one.contains("Страница 1"));
    assert!(back_one.contains("Найдено книг: 47"));
    assert!(navigation_href(&back_one, "prev").is_none());

    for invalid in [
        "/catalog?page=2",
        "/catalog?before=wrong&page=1",
        "/catalog?after=1&before=2&page=2",
        "/catalog?after=00&page=2",
        "/catalog?after=1&page=1",
    ] {
        assert_eq!(get(app.clone(), invalid).await.0, StatusCode::BAD_REQUEST, "{invalid}");
    }
    pool.close().await;
}
