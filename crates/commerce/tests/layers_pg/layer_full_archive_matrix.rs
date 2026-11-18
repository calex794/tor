//! D-05: four chapters in one ordinarily removed Book, with independent
//! subscription-origin rights for the old, renewed, expired and new buyer.
//! This is a PG-only entitlement/TOC matrix; S3 and genuine deletion HTTP
//! are exercised by the separate required paid-private-e2e/MinIO suites.
use super::*;
use tor_commerce::{
    repositories::layer_entitlements::PgLayerChapterEntitlements,
    services::reader_views::{LayerTocAccess, MixedTocCursor, MixedTocSide},
};
use tor_contracts::paid_delivery::{Actor, EntitlementDecision, EntitlementPort};

async fn buyer(pool: &PgPool, suffix: &str) -> i64 {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'D05 chapter matrix','$argon2id$test') RETURNING id",
    )
    .bind(format!("d05-four-{suffix}@example.invalid"))
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES($1,1500,'RUB')",
    )
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
    id
}

async fn four_published_chapters(pool: &PgPool, book: i64) -> Vec<(i64, i64)> {
    let mut editions = Vec::new();
    for number in 1..=4_i32 {
        let (access, price) = if number <= 2 { ("free", 0) } else { ("paid", 50) };
        let chapter: i64 = sqlx::query_scalar(
            "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
             VALUES($1,$2,$3,'public',$4,$5) RETURNING id",
        )
        .bind(book)
        .bind(number)
        .bind(format!("Layer archived matrix chapter {number}"))
        .bind(access)
        .bind(price)
        .fetch_one(pool)
        .await
        .unwrap();
        let version: i64 = sqlx::query_scalar(
            "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
             VALUES($1,1,$2) RETURNING id",
        )
        .bind(chapter)
        .bind(format!("Exact historical chapter number {number}"))
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)",
        )
        .bind(chapter)
        .bind(version)
        .execute(pool)
        .await
        .unwrap();
        editions.push((chapter, version));
    }
    editions
}

fn purchase(
    actor: i64,
    layer: i64,
    offer: i64,
    version: i64,
    key: char,
) -> LayerCheckoutCommand {
    LayerCheckoutCommand {
        actor: id(actor),
        wallet: WalletId::new(actor).unwrap(),
        layer: id(layer),
        offer: id(offer),
        version: id(version),
        expected_price: MoneyMinor::new(90).unwrap(),
        key: IdempotencyKey::parse(&format!("{}8", key.to_string().repeat(42))).unwrap(),
    }
}

async fn assert_old_four(
    rights: &PgLayerChapterEntitlements,
    old: i64,
    editions: &[(i64, i64)],
) {
    for &(chapter, version) in &editions[..2] {
        assert_eq!(
            rights.archived_free_version(id(old), id(chapter)).await.unwrap(),
            Some(id(version)),
        );
    }
    for &(chapter, version) in &editions[2..] {
        assert_eq!(
            rights
                .resolve_entitlement(
                    Actor { user_id: id(old) },
                    id(chapter),
                    None,
                )
                .await
                .unwrap(),
            EntitlementDecision::Allowed {
                pinned: tor_contracts::paid_delivery::ChapterVersionRef {
                    chapter_id: id(chapter),
                    version_id: id(version),
                },
            },
        );
    }
    // A private paid-chapter anchor must enumerate the two frozen FREE rows
    // and the other paid row without fabricating direct purchase history.
    let (anchor, _) = editions[2];
    let before = rights
        .layer_toc_window(
            id(old),
            id(anchor),
            MixedTocCursor {
                chapter_no: 3,
                chapter_id: anchor,
            },
            MixedTocSide::Before,
            10,
        )
        .await
        .unwrap();
    assert_eq!(before.chapters.len(), 2);
    assert!(before
        .chapters
        .iter()
        .all(|row| row.access == LayerTocAccess::ArchivedFree));
    assert!(before
        .chapters
        .iter()
        .all(|row| editions[..2].iter().any(|&(chapter, _)| chapter == row.chapter_id)));
    let after = rights
        .layer_toc_window(
            id(old),
            id(anchor),
            MixedTocCursor {
                chapter_no: 3,
                chapter_id: anchor,
            },
            MixedTocSide::After,
            10,
        )
        .await
        .unwrap();
    assert_eq!(after.chapters.len(), 1);
    assert_eq!(after.chapters[0].chapter_id, editions[3].0);
    assert_eq!(after.chapters[0].access, LayerTocAccess::Subscription);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d05_four_chapter_archived_layer_preserves_only_predeletion_origin() {
    let (pool, layer_repo, owner, old, deleted_book) =
        setup("four-chapter-term-matrix").await;
    let editions = four_published_chapters(&pool, deleted_book).await;
    let surviving_book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
         VALUES($1,'ru','Surviving book after deletion','public',$2,'basic')
         RETURNING id",
    )
    .bind("ci:d05-four-chapter-survivor")
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();

    let layer = layer_repo
        .create_layer(id(owner), "Four chapter archive matrix")
        .await
        .unwrap();
    layer_repo
        .add_book(id(owner), id(layer), id(deleted_book))
        .await
        .unwrap();
    let original_version = layer_repo
        .add_book(id(owner), id(layer), id(surviving_book))
        .await
        .unwrap();
    let offer = layer_repo
        .publish_offer(id(owner), id(layer), 90, false)
        .await
        .unwrap();
    let first = layer_repo
        .buy(&purchase(old, layer, offer, original_version, 'J'))
        .await
        .unwrap();
    let expiring = buyer(&pool, "expiring").await;
    let expired_soon = layer_repo
        .buy(&purchase(expiring, layer, offer, original_version, 'K'))
        .await
        .unwrap();
    // Lifetime uses an independent, non-exclusive Layer offer. A hard expiry
    // on the 30-day actors must not erase its permanent historic origin.
    let lifetime_buyer = buyer(&pool, "lifetime").await;
    let lifetime_layer = layer_repo
        .create_layer(id(owner), "Four chapter lifetime archive")
        .await
        .unwrap();
    layer_repo
        .add_book(id(owner), id(lifetime_layer), id(deleted_book))
        .await
        .unwrap();
    let lifetime_version = layer_repo
        .add_book(id(owner), id(lifetime_layer), id(surviving_book))
        .await
        .unwrap();
    let lifetime_offer = layer_repo
        .publish_offer(id(owner), id(lifetime_layer), 90, true)
        .await
        .unwrap();
    let lifetime = layer_repo
        .buy(&purchase(
            lifetime_buyer,
            lifetime_layer,
            lifetime_offer,
            lifetime_version,
            'P',
        ))
        .await
        .unwrap();
    assert!(sqlx::query_scalar::<_, bool>(
        "SELECT expires_at IS NULL FROM commerce_layer_terms WHERE id=$1",
    )
    .bind(lifetime.term_id)
    .fetch_one(&pool)
    .await
    .unwrap());

    // This PG fixture models the verified SQL state of the actual owner
    // request. A separate test uses the real password/session/moderator API.
    let mut tx = pool.begin().await.unwrap();
    let deletion_revision: i64 = sqlx::query_scalar(
        "UPDATE books SET lifecycle_status='pending_deletion',
           lifecycle_revision=lifecycle_revision+1,visibility='draft'
         WHERE id=$1 RETURNING lifecycle_revision",
    )
    .bind(deleted_book)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_ordinary_deletion_requests
          (book_id,request_revision,owner_user_id,previous_lifecycle_status,previous_visibility)
         VALUES($1,$2,$3,'in_progress','public')",
    )
    .bind(deleted_book)
    .bind(deletion_revision)
    .bind(owner)
    .execute(&mut *tx)
    .await
    .unwrap();
    for &(chapter, version) in &editions[..2] {
        sqlx::query(
            "INSERT INTO book_layer_free_archive_versions
             (book_id,request_revision,chapter_id,version_id)
             VALUES($1,$2,$3,$4)",
        )
        .bind(deleted_book)
        .bind(deletion_revision)
        .bind(chapter)
        .bind(version)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    sqlx::query(
        "UPDATE books SET lifecycle_status='ordinary_deleted',
         ordinary_deleted_at=clock_timestamp(),
         lifecycle_revision=lifecycle_revision+1 WHERE id=$1",
    )
    .bind(deleted_book)
    .execute(&pool)
    .await
    .unwrap();

    let rights = PgLayerChapterEntitlements::new(pool.clone());
    assert_old_four(&rights, old, &editions).await;
    assert_old_four(&rights, lifetime_buyer, &editions).await;
    // New checkout must never sell stale composition or include deleted book
    // in economics, even though a surviving Book remains in the same Layer.
    let late = buyer(&pool, "late").await;
    assert!(layer_repo
        .buy(&purchase(late, layer, offer, original_version, 'L'))
        .await
        .is_err());
    let clean_version = layer_repo
        .remove_book(id(owner), id(layer), id(deleted_book))
        .await
        .unwrap();
    assert_ne!(clean_version, original_version);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM commerce_layer_version_books
             WHERE layer_version_id=$1 AND book_id=$2",
        )
        .bind(clean_version)
        .bind(deleted_book)
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    let late_term = layer_repo
        .buy(&purchase(late, layer, offer, clean_version, 'M'))
        .await
        .unwrap();
    let renewed = layer_repo
        .buy(&purchase(old, layer, offer, clean_version, 'N'))
        .await
        .unwrap();
    assert_ne!(first.term_id, renewed.term_id);
    assert_old_four(&rights, old, &editions).await;
    for &(chapter, _) in &editions[..2] {
        assert_eq!(rights.archived_free_version(id(late), id(chapter)).await.unwrap(), None);
    }
    for &(chapter, _) in &editions[2..] {
        assert!(matches!(
            rights
                .resolve_entitlement(Actor { user_id: id(late) }, id(chapter), None)
                .await
                .unwrap(),
            EntitlementDecision::Denied { .. }
        ));
    }
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM commerce_layer_term_share_snapshots
             WHERE term_id=$1 AND book_id=$2",
        )
        .bind(late_term.term_id)
        .bind(deleted_book)
        .fetch_one(&pool)
        .await
        .unwrap(),
        0
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM library_purchased_chapters
             WHERE user_id=$1 AND chapter_id = ANY($2)",
        )
        .bind(old)
        .bind(editions[2..].iter().map(|(chapter, _)| *chapter).collect::<Vec<_>>())
        .fetch_one(&pool)
        .await
        .unwrap(),
        0,
        "Layer membership must not mint direct purchases"
    );

    // Both time-bounded terms of a different pre-deletion buyer can expire
    // without affecting the continuous buyer's same immutable old origin.
    sqlx::query(
        "UPDATE commerce_layer_terms
         SET started_at=started_at-interval '45 days',
             expires_at=expires_at-interval '45 days'
         WHERE id=$1",
    )
    .bind(expired_soon.term_id)
    .execute(&pool)
    .await
    .unwrap();
    for &(chapter, _) in &editions[..2] {
        assert_eq!(rights.archived_free_version(id(expiring), id(chapter)).await.unwrap(), None);
    }
    for &(chapter, _) in &editions[2..] {
        assert!(matches!(
            rights
                .resolve_entitlement(Actor { user_id: id(expiring) }, id(chapter), None)
                .await
                .unwrap(),
            EntitlementDecision::Denied { .. }
        ));
    }
    assert_old_four(&rights, old, &editions).await;
    assert_old_four(&rights, lifetime_buyer, &editions).await;
    pool.close().await;
}
