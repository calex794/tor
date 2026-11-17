//! D-05: historical free-version identity requires a genuine paid Layer origin.
//! The separate B test checks the owner-request snapshot transaction.
use super::*;
use tor_commerce::repositories::layer_entitlements::PgLayerChapterEntitlements;
use tor_commerce::services::reader_views::{LayerTocAccess, MixedTocCursor, MixedTocSide};

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_deleted_book_free_version_requires_old_term_and_frozen_publication() {
    let (pool, layers, owner, buyer, book) = setup("free-archive-identity").await;
    let (layer, version, offer_id) = offer(&layers, owner, book, 200, false).await;
    let free: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Free V1','public','free',0) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let v1: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'historical free V1') RETURNING id",
    )
    .bind(free)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(free)
        .bind(v1)
        .execute(&pool)
        .await
        .unwrap();
    // A real PAID anchor is needed to page the Layer TOC. The FREE row
    // must retain its historical snapshot even if the public V2 later moves.
    let paid_chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,2,'Paid anchor','public','paid',50) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let paid_version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Paid chapter V1') RETURNING id",
    )
    .bind(paid_chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(paid_chapter)
        .bind(paid_version)
        .execute(&pool)
        .await
        .unwrap();
    let resolver = PgLayerChapterEntitlements::new(pool.clone());
    assert_eq!(
        resolver
            .archived_free_version(id(buyer), id(free))
            .await
            .unwrap(),
        None
    );
    let paid = layers
        .buy(&cmd(buyer, layer, offer_id, version, 200, 'F'))
        .await
        .unwrap();
    assert_eq!(
        resolver
            .archived_free_version(id(buyer), id(free))
            .await
            .unwrap(),
        None
    );

    // An ordinary deletion without a matching immutable owner request is
    // never enough to create a private archival right.
    let mut tx = pool.begin().await.unwrap();
    let revision: i64 = sqlx::query_scalar(
        "UPDATE books SET lifecycle_status='pending_deletion',visibility='draft',
         lifecycle_revision=lifecycle_revision+1
         WHERE id=$1 RETURNING lifecycle_revision",
    )
    .bind(book)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_ordinary_deletion_requests
         (book_id,request_revision,owner_user_id,previous_lifecycle_status,previous_visibility)
         VALUES($1,$2,$3,'in_progress','public')",
    )
    .bind(book)
    .bind(revision)
    .bind(owner)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_layer_free_archive_versions
         (book_id,request_revision,chapter_id,version_id)
         VALUES($1,$2,$3,$4)",
    )
    .bind(book)
    .bind(revision)
    .bind(free)
    .bind(v1)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        resolver
            .archived_free_version(id(buyer), id(free))
            .await
            .unwrap(),
        Some(id(v1))
    );
    assert_eq!(
        resolver
            .archived_free_version(id(owner), id(free))
            .await
            .unwrap(),
        None,
        "Book ownership is not proof of payment"
    );
    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'future free V2') RETURNING id",
    )
    .bind(free)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(free)
        .bind(v2)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        resolver
            .archived_free_version(id(buyer), id(free))
            .await
            .unwrap(),
        Some(id(v1)),
        "the public CURRENT V2 must not rewrite frozen V1"
    );
    sqlx::query(
        "UPDATE books SET lifecycle_status='ordinary_deleted',
         lifecycle_revision=lifecycle_revision+1,
         ordinary_deleted_at=clock_timestamp() WHERE id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        resolver
            .archived_free_version(id(buyer), id(free))
            .await
            .unwrap(),
        Some(id(v1))
    );

    // The visible CURRENT publication is not archival authority. The TOC
    // still resolves the frozen FREE V1 and routes it to private reading,
    // even after the public chapter publication pointer disappears.
    sqlx::query("DELETE FROM chapter_publications WHERE chapter_id=$1")
        .bind(free)
        .execute(&pool)
        .await
        .unwrap();
    let window = resolver
        .layer_toc_window(
            id(buyer),
            id(paid_chapter),
            MixedTocCursor {
                chapter_no: 2,
                chapter_id: paid_chapter,
            },
            MixedTocSide::Before,
            10,
        )
        .await
        .unwrap();
    assert_eq!(window.chapters.len(), 1);
    assert_eq!(window.chapters[0].chapter_id, free);
    assert_eq!(window.chapters[0].access, LayerTocAccess::ArchivedFree);
    assert!(
        resolver
            .layer_toc_window(
                id(owner),
                id(paid_chapter),
                MixedTocCursor {
                    chapter_no: 2,
                    chapter_id: paid_chapter,
                },
                MixedTocSide::Before,
                10,
            )
            .await
            .is_err(),
        "book owner without a paid Layer term cannot list private archive"
    );

    sqlx::query("UPDATE books SET visibility_status='blocked' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        resolver
            .archived_free_version(id(buyer), id(free))
            .await
            .unwrap(),
        None
    );
    sqlx::query("UPDATE books SET visibility_status='active',access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        resolver
            .archived_free_version(id(buyer), id(free))
            .await
            .unwrap(),
        None
    );
    sqlx::query("UPDATE users SET adult_enabled=TRUE WHERE id=$1")
        .bind(buyer)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        resolver
            .archived_free_version(id(buyer), id(free))
            .await
            .unwrap(),
        Some(id(v1))
    );

    sqlx::query(
        "INSERT INTO commerce_layer_term_revocations
           (term_id,reason,reversal_reference)
         VALUES($1,'legal_block',$2)",
    )
    .bind(paid.term_id)
    .bind(format!("ci:free-archive-revoke:{}", paid.term_id))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        resolver
            .archived_free_version(id(buyer), id(free))
            .await
            .unwrap(),
        None
    );
}
