//! D-05 chapter rights: real PostgreSQL, never a synthetic book-level grant.
use super::*;
use tor_commerce::repositories::layer_entitlements::{
    PgLayerChapterEntitlements, PgReaderEntitlements,
};
use tor_contracts::paid_delivery::{
    Actor, ChapterVersionRef, DenialReason, EntitlementDecision, EntitlementPort,
};

async fn paid_chapter(pool: &PgPool, book: i64, number: i32) -> (i64, i64) {
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,$2,'D05 paid chapter','public','paid',75) RETURNING id",
    )
    .bind(book)
    .bind(number)
    .fetch_one(pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Paid V1') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(pool)
        .await
        .unwrap();
    (chapter, version)
}

fn allow(chapter: i64, version: i64) -> EntitlementDecision {
    EntitlementDecision::Allowed {
        pinned: ChapterVersionRef {
            chapter_id: id(chapter),
            version_id: id(version),
        },
    }
}

async fn decision(
    repo: &dyn EntitlementPort,
    actor: i64,
    chapter: i64,
    purchase: Option<i64>,
) -> EntitlementDecision {
    repo.resolve_entitlement(Actor { user_id: id(actor) }, id(chapter), purchase.map(id))
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_layer_current_chapter_requires_active_term_and_current_publication() {
    let (pool, repo, owner, buyer, book) = setup("chapter-current").await;
    let (layer, version, offer_id) = offer(&repo, owner, book, 250, false).await;
    let (chapter, published_v1) = paid_chapter(&pool, book, 1).await;
    let resolver = PgLayerChapterEntitlements::new(pool.clone());

    assert_eq!(
        decision(&resolver, buyer, chapter, None).await,
        EntitlementDecision::Denied {
            reason: DenialReason::NotPurchased
        }
    );
    repo.buy(&cmd(buyer, layer, offer_id, version, 250, 'A'))
        .await
        .unwrap();
    assert_eq!(
        decision(&resolver, buyer, chapter, None).await,
        allow(chapter, published_v1)
    );
    assert_eq!(
        decision(&resolver, owner, chapter, None).await,
        EntitlementDecision::Denied {
            reason: DenialReason::NotPurchased
        }
    );

    // Future publication and unpublished content are never revealed by
    // the book-level Layer right.
    sqlx::query("UPDATE chapters SET reader_released_at=now()+INTERVAL '1 day' WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        decision(&resolver, buyer, chapter, None).await,
        EntitlementDecision::Denied {
            reason: DenialReason::Hidden
        }
    );
    sqlx::query("UPDATE chapters SET reader_released_at=NULL WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();

    let published_v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'Paid V2') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(chapter)
        .bind(published_v2)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        decision(&resolver, buyer, chapter, None).await,
        allow(chapter, published_v2),
        "Layer must follow current published V2, not pin historical V1"
    );

    sqlx::query("UPDATE commerce_layers SET status='hidden' WHERE id=$1")
        .bind(layer)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        decision(&resolver, buyer, chapter, None).await,
        allow(chapter, published_v2),
        "hidden storefront must preserve an already paid right"
    );
    sqlx::query("UPDATE commerce_layers SET status='blocked' WHERE id=$1")
        .bind(layer)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        decision(&resolver, buyer, chapter, None).await,
        allow(chapter, published_v2),
        "blocking new Layer sales must not revoke an already paid term"
    );
    sqlx::query("UPDATE chapters SET admin_hold=true WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        decision(&resolver, buyer, chapter, None).await,
        EntitlementDecision::Denied {
            reason: DenialReason::Hidden
        },
        "a chapter hold overrides paid Layer membership"
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_layer_grant_never_bypasses_revocation_or_direct_purchase_identity() {
    let (pool, repo, owner, buyer, book) = setup("chapter-revocation").await;
    let (layer, version, offer_id) = offer(&repo, owner, book, 250, true).await;
    let (chapter, published) = paid_chapter(&pool, book, 1).await;
    let receipt = repo
        .buy(&cmd(buyer, layer, offer_id, version, 250, 'A'))
        .await
        .unwrap();
    let layer_resolver = PgLayerChapterEntitlements::new(pool.clone());
    let combined = PgReaderEntitlements::new(pool.clone());
    assert_eq!(
        decision(&combined, buyer, chapter, None).await,
        allow(chapter, published)
    );
    assert_eq!(
        decision(&layer_resolver, buyer, chapter, Some(999)).await,
        EntitlementDecision::Denied {
            reason: DenialReason::NotPurchased
        }
    );
    assert_eq!(
        decision(&combined, buyer, chapter, Some(999)).await,
        EntitlementDecision::Denied {
            reason: DenialReason::NotPurchased
        },
        "a fake direct purchase ID cannot fall through to Layer"
    );

    sqlx::query(
        "INSERT INTO commerce_layer_term_revocations(term_id,reason,reversal_reference)
         VALUES($1,'legal_block','d05-chapter-revoke-lifetime')",
    )
    .bind(receipt.term_id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        decision(&combined, buyer, chapter, None).await,
        EntitlementDecision::Denied {
            reason: DenialReason::NotPurchased
        }
    );
    pool.close().await;
}

/// C-12/S065: the *same* buyer owns both a direct V1 and a Layer CURRENT
/// entitlement. Two paid rights must not promote V1 to V2, share provenance
/// or revoke one another. All facts are committed to disposable PostgreSQL.
#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_c12_direct_pin_and_layer_current_rights_are_independent() {
    use tor_commerce::{
        domain::money::{CheckoutRepository, PurchaseCommand},
        repositories::{checkout::PgCheckout, entitlements::PgDirectEntitlements},
    };

    let (pool, layers, owner, buyer, book) = setup("c12-dual-rights").await;
    let (chapter, v1) = paid_chapter(&pool, book, 1).await;
    let direct_purchase = PgCheckout::new(pool.clone())
        .checkout_once(
            &PurchaseCommand::new(
                id(buyer),
                WalletId::new(buyer).unwrap(),
                ChapterVersionRef {
                    chapter_id: id(chapter),
                    version_id: id(v1),
                },
                MoneyMinor::new(75).unwrap(),
                IdempotencyKey::parse(&format!("{}8", "C".repeat(42))).unwrap(),
            )
            .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(direct_purchase.balance_after.get(), 1425);
    let purchase_id = direct_purchase.purchase_id.get();
    let direct = PgDirectEntitlements::new(pool.clone());
    let layer = PgLayerChapterEntitlements::new(pool.clone());
    let combined = PgReaderEntitlements::new(pool.clone());

    assert_eq!(decision(&direct, buyer, chapter, Some(purchase_id)).await, allow(chapter, v1));
    assert_eq!(decision(&combined, buyer, chapter, None).await, allow(chapter, v1));
    assert_eq!(
        decision(&layer, buyer, chapter, None).await,
        EntitlementDecision::Denied {
            reason: DenialReason::NotPurchased
        },
        "a direct purchase is never a Layer subscription"
    );

    let v2: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,2,'Layer CURRENT V2') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE chapter_publications SET version_id=$2 WHERE chapter_id=$1")
        .bind(chapter)
        .bind(v2)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        decision(&direct, buyer, chapter, Some(purchase_id)).await,
        allow(chapter, v1),
        "publishing V2 cannot upgrade an existing direct purchase"
    );

    let (layer_id, version, offer_id) = offer(&layers, owner, book, 250, true).await;
    let term = layers
        .buy(&cmd(buyer, layer_id, offer_id, version, 250, 'L'))
        .await
        .unwrap();
    assert_eq!(term.balance_after_minor, 1175);
    assert_eq!(decision(&layer, buyer, chapter, None).await, allow(chapter, v2));
    assert_eq!(
        decision(&direct, buyer, chapter, Some(purchase_id)).await,
        allow(chapter, v1)
    );
    assert_eq!(
        decision(&combined, buyer, chapter, None).await,
        allow(chapter, v1),
        "without a source override a direct V1 takes priority"
    );
    assert_eq!(
        decision(&combined, buyer, chapter, Some(purchase_id)).await,
        allow(chapter, v1),
        "an exact purchase ID must never resolve Layer V2"
    );
    assert_eq!(
        decision(&combined, buyer, chapter, Some(purchase_id + 1)).await,
        EntitlementDecision::Denied {
            reason: DenialReason::NotPurchased
        },
        "a forged direct ID must not fall back to an owned Layer"
    );
    assert_eq!(
        decision(&layer, buyer, chapter, Some(purchase_id)).await,
        EntitlementDecision::Denied {
            reason: DenialReason::NotPurchased
        },
        "Layer explicitly refuses direct purchase identities"
    );
    let (direct_entries, layer_entries): (i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT count(*) FROM reader_wallet_entries
            WHERE user_id=$1 AND kind='chapter_purchase'),
           (SELECT count(*) FROM reader_wallet_entries
            WHERE user_id=$1 AND kind='layer_purchase')",
    )
    .bind(buyer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((direct_entries, layer_entries), (1, 1));

    sqlx::query("UPDATE chapters SET admin_hold=true WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        decision(&combined, buyer, chapter, None).await,
        EntitlementDecision::Denied {
            reason: DenialReason::Locked
        },
        "Layer must not override a hard denial from the direct reader"
    );
    assert_eq!(
        decision(&layer, buyer, chapter, None).await,
        EntitlementDecision::Denied {
            reason: DenialReason::Hidden
        }
    );
    sqlx::query("UPDATE chapters SET admin_hold=false WHERE id=$1")
        .bind(chapter)
        .execute(&pool)
        .await
        .unwrap();

    // Canonical direct refund removes V1 but must not confiscate a distinct
    // paid Layer term. An explicit revoked purchase ID still fails closed.
    sqlx::query(
        "INSERT INTO chapter_purchase_revocations
         (purchase_id,reversal_reference,reason) VALUES($1,$2,'refund')",
    )
    .bind(purchase_id)
    .bind(format!("ci:c12-direct-layer-refund:{purchase_id}"))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        decision(&direct, buyer, chapter, Some(purchase_id)).await,
        EntitlementDecision::Denied {
            reason: DenialReason::Refunded
        }
    );
    assert_eq!(
        decision(&combined, buyer, chapter, Some(purchase_id)).await,
        EntitlementDecision::Denied {
            reason: DenialReason::Refunded
        },
        "an explicit revoked V1 cannot be replaced by the Layer V2"
    );
    assert_eq!(
        decision(&combined, buyer, chapter, None).await,
        allow(chapter, v2),
        "a still-active Layer should independently authorize CURRENT V2"
    );

    sqlx::query(
        "INSERT INTO commerce_layer_term_revocations
         (term_id,reason,reversal_reference) VALUES($1,'legal_block',$2)",
    )
    .bind(term.term_id)
    .bind(format!("ci:c12-dual-term-revoked:{}", term.term_id))
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        decision(&layer, buyer, chapter, None).await,
        EntitlementDecision::Denied {
            reason: DenialReason::NotPurchased
        }
    );
    assert_eq!(
        decision(&combined, buyer, chapter, None).await,
        EntitlementDecision::Denied {
            reason: DenialReason::Refunded
        },
        "once both independent rights are revoked no paid edition is readable"
    );
    pool.close().await;
}
