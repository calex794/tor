//! S020: real public book page must show only explicitly linked, live-public
//! targets; a subsequent moderation hold/draft transition revokes visibility.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use sqlx::PgPool;
use std::time::{SystemTime, UNIX_EPOCH};
use tor_books::repositories::PgCatalog;
use tower::ServiceExt;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL and real reader"]
async fn s020_public_related_books_are_reciprocal_and_revocable() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable tor_ci URL");
    assert!(url.ends_with("/tor_ci"), "never run on production");
    let pool = PgPool::connect(&url).await.expect("PG");
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut ids = Vec::new();
    for (suffix, title, visibility, held) in [
        ("source", format!("S020 Главная {stamp}"), "public", false),
        (
            "target",
            format!("<img src=x onerror=alert(1)> Том {stamp}"),
            "public",
            false,
        ),
        (
            "hidden",
            format!("S020 Скрытый том {stamp}"),
            "draft",
            false,
        ),
        (
            "held",
            format!("S020 Заблокированный том {stamp}"),
            "public",
            true,
        ),
        (
            "unlinked",
            format!("S020 Посторонняя книга {stamp}"),
            "public",
            false,
        ),
    ] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO books(source_identity,target_language,title,visibility,admin_hold)
             VALUES($1,'ru',$2,$3,$4) RETURNING id",
        )
        .bind(format!("ci:s020:public:{suffix}:{stamp}"))
        .bind(title)
        .bind(visibility)
        .bind(held)
        .fetch_one(&pool)
        .await
        .expect("insert fixture");
        ids.push(id);
    }
    let (source, target, hidden, held, unlinked) = (ids[0], ids[1], ids[2], ids[3], ids[4]);
    for target_id in [target, hidden, held] {
        sqlx::query(
            "INSERT INTO book_related_links(smaller_book_id,larger_book_id)
             VALUES($1,$2)",
        )
        .bind(source.min(target_id))
        .bind(source.max(target_id))
        .execute(&pool)
        .await
        .expect("seed explicit relation");
    }
    let catalog = PgCatalog::connect(&url).await.expect("tor_app catalog");
    let reader = tor_reading::repositories::PgReader::connect(&url)
        .await
        .expect("tor_app reader");
    let app = tor_web::router_with_postgres_reading(catalog, reader);

    async fn page(app: axum::Router, id: i64) -> (StatusCode, String) {
        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/books/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let body = to_bytes(res.into_body(), 256 * 1024).await.unwrap();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }
    let (status, html) = page(app.clone(), source).await;
    assert_eq!(status, StatusCode::OK, "{html}");
    assert!(html.contains("Связанные книги"), "{html}");
    assert!(html.contains(&format!("href=\"/books/{target}\"")));
    assert!(html.contains("&lt;img src=x onerror=alert(1)&gt;"));
    assert!(!html.contains("<img src=x onerror=alert(1)>"));
    assert!(!html.contains(&format!("href=\"/books/{hidden}\"")));
    assert!(!html.contains(&format!("href=\"/books/{held}\"")));
    assert!(!html.contains(&format!("href=\"/books/{unlinked}\"")));
    assert!(!html.contains("Скрытый том"));
    assert!(!html.contains("Заблокированный том"));

    let (back_status, back) = page(app.clone(), target).await;
    assert_eq!(back_status, StatusCode::OK, "{back}");
    assert!(back.contains(&format!("href=\"/books/{source}\"")));

    let (unlinked_status, unlinked_page) = page(app.clone(), unlinked).await;
    assert_eq!(unlinked_status, StatusCode::OK);
    assert!(!unlinked_page.contains("Связанные книги"));

    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(target)
        .execute(&pool)
        .await
        .unwrap();
    let (status, held_html) = page(app.clone(), source).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!held_html.contains(&format!("href=\"/books/{target}\"")));
    assert!(!held_html.contains("Том"));
    assert_eq!(page(app.clone(), target).await.0, StatusCode::NOT_FOUND);

    sqlx::query("UPDATE books SET admin_hold=FALSE,visibility='draft' WHERE id=$1")
        .bind(target)
        .execute(&pool)
        .await
        .unwrap();
    let (status, draft_html) = page(app.clone(), source).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!draft_html.contains(&format!("href=\"/books/{target}\"")));

    // Even after restoring the target, the source cannot link from a draft.
    sqlx::query("UPDATE books SET visibility='public' WHERE id=$1")
        .bind(target)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE books SET visibility='draft' WHERE id=$1")
        .bind(source)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(page(app.clone(), source).await.0, StatusCode::NOT_FOUND);
    pool.close().await;
}
