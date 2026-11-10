//! Layer CURRENT TOC: 205-chapter PG keyset, age, hidden and revoked rights.
//! All tests require the disposable tor_ci DB; never use a production source.
use super::*;
use tor_commerce::{
    repositories::layer_entitlements::PgLayerChapterEntitlements,
    services::reader_views::{LayerTocAccess, LayerTocAnchor, MixedTocCursor, MixedTocSide},
};
use tor_contracts::paid_delivery::DeliveryError;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_layer_toc_205_pages_canonical_entitlement_and_revocation() {
    let (pool, layers, owner, buyer, book) = setup("toc205").await;
    let (layer, version, offer_id) = offer(&layers, owner, book, 250, false).await;

    sqlx::query(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         SELECT $1,n,'Глава '||n,'public',
             CASE WHEN n%2=0 THEN 'free' ELSE 'paid' END,
             CASE WHEN n%2=0 THEN 0 ELSE 75 END
         FROM generate_series(1,205) AS n",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         SELECT id,1,'PRIVATE-VERSION-DO-NOT-EXPOSE'
         FROM chapters WHERE book_id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_publications(chapter_id,version_id)
         SELECT c.id,v.id FROM chapters c
         JOIN chapter_versions v ON v.chapter_id=c.id
         WHERE c.book_id=$1",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    let anchor: i64 =
        sqlx::query_scalar("SELECT id FROM chapters WHERE book_id=$1 AND chapter_no=1")
            .bind(book)
            .fetch_one(&pool)
            .await
            .unwrap();
    let last: i64 =
        sqlx::query_scalar("SELECT id FROM chapters WHERE book_id=$1 AND chapter_no=205")
            .bind(book)
            .fetch_one(&pool)
            .await
            .unwrap();
    // Additional private draft and unreleased rows must not be enumerated.
    let hidden: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,206,'SECRET-DRAFT-TOC','draft','paid',75) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let future: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount,reader_released_at)
         VALUES($1,207,'SECRET-FUTURE-TOC','public','paid',75,now()+INTERVAL '2 days')
         RETURNING id",
    )
    .bind(book).fetch_one(&pool).await.unwrap();
    sqlx::query(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'PRIVATE-FUTURE-TOC'),($2,1,'PRIVATE-DRAFT-TOC')",
    )
    .bind(future)
    .bind(hidden)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_publications(chapter_id,version_id)
         SELECT c.id,v.id FROM chapters c
         JOIN chapter_versions v ON v.chapter_id=c.id
         WHERE c.id IN ($1,$2)",
    )
    .bind(future)
    .bind(hidden)
    .execute(&pool)
    .await
    .unwrap();

    let toc = PgLayerChapterEntitlements::new(pool.clone());
    assert_eq!(
        toc.toc_anchor(id(buyer), id(anchor)).await,
        Err(DeliveryError::Denied),
        "Layer storefront is not an entitlement"
    );
    let term = layers
        .buy(&cmd(buyer, layer, offer_id, version, 250, 'L'))
        .await
        .unwrap();
    let actual_anchor = toc.toc_anchor(id(buyer), id(anchor)).await.unwrap();
    assert_eq!(
        actual_anchor,
        LayerTocAnchor {
            book_id: book,
            chapter_id: anchor,
            chapter_no: 1,
            chapter_title: "Глава 1".into(),
            book_title: "D05 own book".into(),
        }
    );
    assert_eq!(
        toc.toc_anchor(id(owner), id(anchor)).await,
        Err(DeliveryError::Denied)
    );

    let mut cursor = MixedTocCursor {
        chapter_no: 1,
        chapter_id: anchor,
    };
    let mut observed = Vec::new();
    for _ in 0..5 {
        let page = toc
            .layer_toc_window(id(buyer), id(anchor), cursor, MixedTocSide::After, 60)
            .await
            .unwrap();
        assert!(page.chapters.len() <= 60);
        for chapter in &page.chapters {
            observed.push(chapter.chapter_no);
            assert_eq!(
                chapter.access,
                if chapter.chapter_no % 2 == 0 {
                    LayerTocAccess::PublicFree
                } else {
                    LayerTocAccess::Subscription
                }
            );
            assert!(!chapter.title.contains("SECRET"));
        }
        if let Some(next) = page.next_cursor {
            cursor = MixedTocCursor::parse(&next).unwrap();
        } else {
            break;
        }
    }
    assert_eq!(observed, (2..=205).collect::<Vec<_>>());
    let before = toc
        .layer_toc_window(
            id(buyer),
            id(anchor),
            MixedTocCursor {
                chapter_no: 205,
                chapter_id: last,
            },
            MixedTocSide::Before,
            60,
        )
        .await
        .unwrap();
    assert_eq!(before.chapters.len(), 60);
    assert_eq!(before.chapters.first().unwrap().chapter_no, 145);
    assert_eq!(before.chapters.last().unwrap().chapter_no, 204);
    assert!(before.next_cursor.is_some());
    assert_eq!(
        toc.layer_toc_window(
            id(owner),
            id(anchor),
            MixedTocCursor {
                chapter_no: 1,
                chapter_id: anchor
            },
            MixedTocSide::After,
            60,
        )
        .await,
        Err(DeliveryError::Denied)
    );
    assert_eq!(
        toc.layer_toc_window(
            id(buyer),
            id(anchor),
            MixedTocCursor {
                chapter_no: 999,
                chapter_id: anchor
            },
            MixedTocSide::After,
            60,
        )
        .await,
        Err(DeliveryError::Invalid)
    );
    assert_eq!(
        toc.layer_toc_window(
            id(buyer),
            id(anchor),
            MixedTocCursor {
                chapter_no: 1,
                chapter_id: anchor
            },
            MixedTocSide::After,
            0,
        )
        .await,
        Err(DeliveryError::Invalid)
    );
    sqlx::query("UPDATE books SET access_level='adult' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        toc.toc_anchor(id(buyer), id(anchor)).await,
        Err(DeliveryError::Hidden),
        "age must gate even subscribed chapters"
    );
    sqlx::query("UPDATE books SET access_level='basic' WHERE id=$1")
        .bind(book)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO commerce_layer_term_revocations(term_id,reason,reversal_reference)
         VALUES($1,'legal_block',$2)",
    )
    .bind(term.term_id)
    .bind(format!("ci:layer-toc-revocation:{}", term.term_id))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        toc.layer_toc_window(
            id(buyer),
            id(anchor),
            MixedTocCursor {
                chapter_no: 1,
                chapter_id: anchor
            },
            MixedTocSide::After,
            60,
        )
        .await,
        Err(DeliveryError::Denied),
        "a revoked Layer must not page historical metadata"
    );
    pool.close().await;
}
