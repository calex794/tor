//! S020: real public book page must show only explicitly linked, live-public
//! targets; a subsequent moderation hold/draft transition revokes visibility.
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};
use tor_books::repositories::PgCatalog;
use tower::ServiceExt;

fn sql(url: &str, query: &str) -> String {
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let out = Command::new("psql")
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
        .expect("isolated PG fixture");
    assert!(
        out.status.success(),
        "fixture query failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_owned()
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL and real reader"]
async fn s020_public_related_books_are_reciprocal_and_revocable() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable tor_ci URL");
    assert!(url.ends_with("/tor_ci"), "never run on production");
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
        let title_sql = title.replace('\'', "''");
        let id: i64 = sql(
            &url,
            &format!(
                "INSERT INTO books(source_identity,target_language,title,visibility,admin_hold,access_level)
                 VALUES('ci:s020:public:{suffix}:{stamp}','ru','{title_sql}','{visibility}',{held},'basic')
                 RETURNING id"
            ),
        )
        .parse()
        .expect("insert fixture");
        ids.push(id);
    }
    let (source, target, hidden, held, unlinked) = (ids[0], ids[1], ids[2], ids[3], ids[4]);
    for target_id in [target, hidden, held] {
        sql(
            &url,
            &format!(
                "INSERT INTO book_related_links(smaller_book_id,larger_book_id)
                 VALUES({},{})",
                source.min(target_id),
                source.max(target_id)
            ),
        );
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

    // A Book pending deletion is unlisted, even when a public bilateral link
    // was created before the lifecycle transition. Management keeps that link.
    sql(
        &url,
        &format!("UPDATE books SET lifecycle_status='pending_deletion' WHERE id={target}"),
    );
    let (status, unlisted) = page(app.clone(), source).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!unlisted.contains(&format!("href=\"/books/{target}\"")));
    assert!(!unlisted.contains(&format!("Том {stamp}")));
    sql(
        &url,
        &format!("UPDATE books SET lifecycle_status='in_progress' WHERE id={target}"),
    );
    assert!(page(app.clone(), source)
        .await
        .1
        .contains(&format!("href=\"/books/{target}\"")));

    sql(
        &url,
        &format!("UPDATE books SET lifecycle_status='pending_deletion' WHERE id={source}"),
    );
    // This test covers public related-card discovery, NOT direct Book reading.
    // NEwBOO allows a direct Book view during pending_deletion until hard delete;
    // a 404 assertion here would silently canonize the wrong reader policy.
    let (reverse_status, reverse) = page(app.clone(), target).await;
    assert_eq!(reverse_status, StatusCode::OK);
    assert!(!reverse.contains(&format!("href=\"/books/{source}\"")));
    sql(
        &url,
        &format!("UPDATE books SET lifecycle_status='in_progress' WHERE id={source}"),
    );

    let (back_status, back) = page(app.clone(), target).await;
    assert_eq!(back_status, StatusCode::OK, "{back}");
    assert!(back.contains(&format!("href=\"/books/{source}\"")));

    // A completed ordinary deletion is a frozen tombstone, not a public
    // recommendation target. Historical links stay stored for old rights.
    sql(
        &url,
        &format!(
            "UPDATE books SET lifecycle_status='ordinary_deleted',visibility='draft',
             ordinary_deleted_at=clock_timestamp() WHERE id={target}"
        ),
    );
    let (status, ordinary_target) = page(app.clone(), source).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!ordinary_target.contains(&format!("href=\"/books/{target}\"")));
    assert!(!ordinary_target.contains(&format!("Том {stamp}")));
    sql(
        &url,
        &format!(
            "UPDATE books SET lifecycle_status='in_progress',visibility='public',
             ordinary_deleted_at=NULL WHERE id={target}"
        ),
    );
    assert!(page(app.clone(), source)
        .await
        .1
        .contains(&format!("href=\"/books/{target}\"")));

    sql(
        &url,
        &format!(
            "UPDATE books SET lifecycle_status='ordinary_deleted',visibility='draft',
             ordinary_deleted_at=clock_timestamp() WHERE id={source}"
        ),
    );
    // Do not assert source direct-read status: a saved purchase can still
    // have a separate historical-reader entitlement.
    let (reverse_status, ordinary_source) = page(app.clone(), target).await;
    assert_eq!(reverse_status, StatusCode::OK);
    assert!(!ordinary_source.contains(&format!("href=\"/books/{source}\"")));
    sql(
        &url,
        &format!(
            "UPDATE books SET lifecycle_status='in_progress',visibility='public',
             ordinary_deleted_at=NULL WHERE id={source}"
        ),
    );

    let (unlinked_status, unlinked_page) = page(app.clone(), unlinked).await;
    assert_eq!(unlinked_status, StatusCode::OK);
    assert!(!unlinked_page.contains("Связанные книги"));

    sql(
        &url,
        &format!("UPDATE books SET admin_hold=TRUE WHERE id={target}"),
    );
    let (status, held_html) = page(app.clone(), source).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!held_html.contains(&format!("href=\"/books/{target}\"")));
    assert!(!held_html.contains("Том"));
    assert_eq!(page(app.clone(), target).await.0, StatusCode::NOT_FOUND);

    sql(
        &url,
        &format!("UPDATE books SET admin_hold=FALSE,visibility='draft' WHERE id={target}"),
    );
    let (status, draft_html) = page(app.clone(), source).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!draft_html.contains(&format!("href=\"/books/{target}\"")));

    // Even after restoring the target, the source cannot link from a draft.
    sql(
        &url,
        &format!("UPDATE books SET visibility='public' WHERE id={target}"),
    );
    sql(
        &url,
        &format!("UPDATE books SET visibility='draft' WHERE id={source}"),
    );
    assert_eq!(page(app.clone(), source).await.0, StatusCode::NOT_FOUND);
}
