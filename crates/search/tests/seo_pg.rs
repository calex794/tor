//! Real PostgreSQL evidence for the book-side facts consumed by SEO.
//! The production read adapter and sitemap projection are separate SEO-02/06 tasks.
use sqlx::{PgPool, Postgres, Row, Transaction};
use tor_search::domain::seo::{
    SeoBookFacts, SeoBookView, SeoMode, SeoPage, SeoReason, SeoSite, SeoVisibility,
};
use tor_search::services::seo::SeoPolicy;

async fn actual_book_facts(tx: &mut Transaction<'_, Postgres>, book_id: i64) -> SeoBookFacts {
    let row = sqlx::query(
        "SELECT b.visibility, b.admin_hold, b.is_deleted,
         EXISTS(
             SELECT 1 FROM chapters c
             JOIN chapter_publications p ON p.chapter_id = c.id
             WHERE c.book_id = b.id
               AND c.visibility = 'public' AND c.access_level = 'free'
               AND NOT c.admin_hold AND NOT c.is_deleted
               AND (c.reader_released_at IS NULL OR c.reader_released_at <= now())
         ) AS has_public_content
         FROM books b WHERE b.id = $1",
    )
    .bind(book_id)
    .fetch_one(&mut **tx)
    .await
    .unwrap();
    let visibility: String = row.try_get("visibility").unwrap();
    let held: bool = row.try_get("admin_hold").unwrap();
    let deleted: bool = row.try_get("is_deleted").unwrap();
    let visibility = if deleted {
        SeoVisibility::Deleted
    } else if held {
        SeoVisibility::Held
    } else if visibility == "public" {
        SeoVisibility::AnonymousPublic
    } else {
        SeoVisibility::Draft
    };
    SeoBookFacts {
        book_id: book_id as u64,
        visibility,
        has_public_content: row.try_get("has_public_content").unwrap(),
        index_owner: SeoSite::Pandaolate,
        shadow_enabled: false,
        pilot_approved: false,
    }
}

async fn decision(
    tx: &mut Transaction<'_, Postgres>,
    book_id: i64,
) -> tor_search::domain::seo::SeoDecision {
    SeoPolicy::new(SeoSite::Pandaolate, SeoMode::Public).decide(SeoPage::Book {
        book: actual_book_facts(tx, book_id).await,
        view: SeoBookView::Canonical,
    })
}

#[tokio::test]
#[ignore = "requires isolated tor_ci PostgreSQL and migrations"]
async fn f_08_seo_01_pg_publication_and_withdrawal_fail_closed() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database URL");
    let pool = PgPool::connect(&url).await.unwrap();
    let name: String = sqlx::query_scalar("SELECT current_database()")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "tor_ci", "never change production data");
    let mut tx = pool.begin().await.unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility)
         VALUES ('ci:f08:seo01:temporary','ru','SEO-01 fixture','draft')
         RETURNING id",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();

    let draft = decision(&mut tx, book_id).await;
    assert_eq!(draft.reason, SeoReason::BookNotPublic);
    assert!(!draft.sitemap_included);

    sqlx::query("UPDATE books SET visibility='public' WHERE id=$1")
        .bind(book_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let empty = decision(&mut tx, book_id).await;
    assert_eq!(empty.reason, SeoReason::BookHasNoPublicContent);

    let chapter_id: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level)
         VALUES ($1,98765,'SEO chapter','public','free') RETURNING id",
    )
    .bind(book_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let version_id: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES ($1,1,'SEO test content') RETURNING id",
    )
    .bind(chapter_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter_id)
        .bind(version_id)
        .execute(&mut *tx)
        .await
        .unwrap();

    let visible = decision(&mut tx, book_id).await;
    assert!(visible.indexable());
    assert_eq!(
        visible.canonical_path.as_deref(),
        Some(format!("/books/{book_id}").as_str())
    );

    sqlx::query("UPDATE chapters SET reader_released_at=now()+interval '1 day' WHERE id=$1")
        .bind(chapter_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        decision(&mut tx, book_id).await.reason,
        SeoReason::BookHasNoPublicContent
    );

    sqlx::query("UPDATE chapters SET reader_released_at=NULL WHERE id=$1")
        .bind(chapter_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE books SET admin_hold=TRUE WHERE id=$1")
        .bind(book_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        decision(&mut tx, book_id).await.reason,
        SeoReason::BookNotPublic
    );

    sqlx::query("UPDATE books SET admin_hold=FALSE, is_deleted=TRUE WHERE id=$1")
        .bind(book_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert_eq!(
        decision(&mut tx, book_id).await.reason,
        SeoReason::BookNotPublic
    );

    tx.rollback().await.unwrap();
}
