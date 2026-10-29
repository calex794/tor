//! D-06: real PG read-only evidence, never money permission or hidden grant.
use super::*;
use tor_commerce::{
    domain::money::{CheckoutRepository, PurchaseCommand},
    repositories::{
        checkout::PgCheckout,
        direct_exposure::{DirectGrantKind, PgDirectFirstExposure},
        layer_exposure::{LayerGrantKind, PgLayerFirstExposure},
        refund_use_evidence::PgRefundUseEvidence,
        refund_preflight::PgLayerRefundPreflight,
    },
    services::refunds::{
        LayerRefundIntent, LayerRefundService, RefundGateError, UseEvidence,
        UseObservation,
    },
};
use tor_contracts::paid_delivery::ChapterVersionRef;

fn unknown() -> UseObservation {
    UseObservation {
        evidence: UseEvidence::Unknown,
        access_revoked: false,
    }
}

async fn published_paid(pool: &PgPool, book: i64, number: i32) -> (i64, i64) {
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters
         (book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,$2,'D06 use audit','public','paid',75) RETURNING id",
    )
    .bind(book)
    .bind(number)
    .fetch_one(pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'Protected audit V1') RETURNING id",
    )
    .bind(chapter)
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
    (chapter, version)
}

fn no_refund_money(pool: &PgPool, buyer: i64) -> impl std::future::Future<Output = (i64,i64)> + '_ {
    async move {
        sqlx::query_as(
            "SELECT
               (SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1),
               (SELECT count(*) FROM reader_wallet_entries WHERE user_id=$1)",
        )
        .bind(buyer)
        .fetch_one(pool)
        .await
        .unwrap()
    }
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL TEST_DATABASE_URL ending /tor_ci"]
async fn d_06_pg_direct_audit_is_per_purchase_actor_bound_and_not_refund_authority() {
    let (pool, _repo, owner, buyer, book) = setup("refund-use-direct").await;
    let (a, av) = published_paid(&pool, book, 1).await;
    let (b, bv) = published_paid(&pool, book, 2).await;
    let checkout = PgCheckout::new(pool.clone());
    let mut ids = Vec::new();
    for (chapter, version, k) in [(a, av, 'V'), (b, bv, 'W')] {
        let paid = checkout
            .checkout_once(
                &PurchaseCommand::new(
                    id(buyer),
                    WalletId::new(buyer).unwrap(),
                    ChapterVersionRef {
                        chapter_id: id(chapter),
                        version_id: id(version),
                    },
                    MoneyMinor::new(75).unwrap(),
                    IdempotencyKey::parse(&format!("{}8", k.to_string().repeat(42))).unwrap(),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        ids.push(paid.purchase_id.get());
    }
    let (pa, pb) = (ids[0], ids[1]);
    let audit = PgRefundUseEvidence::new(pool.clone());
    let grants = PgDirectFirstExposure::new(pool.clone());
    let expected_money = (1350_i64, 2_i64);
    assert_eq!(no_refund_money(&pool, buyer).await, expected_money);
    assert_eq!(audit.direct(id(buyer), id(pa)).await, Ok(unknown()));
    assert_eq!(audit.direct(id(buyer), id(pb)).await, Ok(unknown()));
    assert_eq!(
        audit.direct(id(owner), id(pa)).await,
        Err(RefundGateError::NotFound),
        "never disclose foreign buyer's receipt",
    );
    assert_eq!(
        audit.direct(id(buyer), id(i64::MAX)).await,
        Err(RefundGateError::NotFound),
    );

    // A private image credential is NOT a verified chapter-text open.
    grants
        .record(id(buyer), id(a), id(av), Some(id(pa)), DirectGrantKind::Asset)
        .await
        .unwrap();
    assert_eq!(
        audit.direct(id(buyer), id(pa)).await.unwrap().evidence,
        UseEvidence::ProtectedGrantOnly,
    );
    assert_eq!(audit.direct(id(buyer), id(pb)).await, Ok(unknown()));
    grants
        .record(id(buyer), id(a), id(av), Some(id(pa)), DirectGrantKind::Chapter)
        .await
        .unwrap();
    assert_eq!(
        audit.direct(id(buyer), id(pa)).await.unwrap().evidence,
        UseEvidence::ChapterTextGrantUnconfirmed,
        "signed text may never have reached the browser",
    );
    grants
        .confirm_rendered(id(buyer), id(a), id(av), id(pa))
        .await
        .unwrap();
    assert_eq!(
        audit.direct(id(buyer), id(pa)).await.unwrap().evidence,
        UseEvidence::BrowserRenderReported,
    );
    assert_eq!(
        audit.direct(id(buyer), id(pb)).await,
        Ok(unknown()),
        "opening A must not mark separate purchase B",
    );

    // Revocation is a separate flag and does NOT imply ledger reversal.
    sqlx::query(
        "INSERT INTO chapter_purchase_revocations
         (purchase_id,reversal_reference,reason)
         VALUES($1,'d06:use-audit-direct-revoke','refund')",
    )
    .bind(pa)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        audit.direct(id(buyer), id(pa)).await.unwrap(),
        UseObservation {
            evidence: UseEvidence::BrowserRenderReported,
            access_revoked: true,
        },
    );
    assert_eq!(audit.direct(id(buyer), id(pb)).await, Ok(unknown()));
    assert_eq!(no_refund_money(&pool, buyer).await, expected_money);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL TEST_DATABASE_URL ending /tor_ci"]
async fn d_06_pg_layer_use_audit_separates_active_p1_from_future_p2() {
    let (pool, layers, owner, buyer, book) = setup("refund-use-layer").await;
    let (chapter, version) = published_paid(&pool, book, 1).await;
    let (layer, composition, offer_id) = offer(&layers, owner, book, 225, false).await;
    let current = layers
        .buy(&cmd(buyer, layer, offer_id, composition, 225, 'X'))
        .await
        .unwrap();
    let future = layers
        .buy(&cmd(buyer, layer, offer_id, composition, 225, 'Y'))
        .await
        .unwrap();
    assert_ne!(current.term_id, future.term_id);
    let audit = PgRefundUseEvidence::new(pool.clone());
    let grants = PgLayerFirstExposure::new(pool.clone());
    let service = LayerRefundService::new(PgLayerRefundPreflight::new(pool.clone()));
    let intent = LayerRefundIntent::new(
        id(buyer),
        id(current.term_id),
        "No delivered text",
        IdempotencyKey::parse(&format!("{}8", "Z".repeat(42))).unwrap(),
    )
    .unwrap();
    let initial_money = no_refund_money(&pool, buyer).await;
    assert_eq!(initial_money, (1050, 2));
    assert_eq!(audit.layer(id(buyer), id(current.term_id)).await, Ok(unknown()));
    assert_eq!(audit.layer(id(buyer), id(future.term_id)).await, Ok(unknown()));
    assert_eq!(
        audit.layer(id(owner), id(current.term_id)).await,
        Err(RefundGateError::NotFound),
    );
    assert_eq!(
        service.observe_use(&intent).await,
        Ok(unknown()),
        "service returns only observation, never financial approval",
    );
    assert_eq!(
        service.preflight(&intent).await,
        Err(RefundGateError::PolicyUnavailable),
    );

    grants
        .record(id(buyer), id(chapter), id(version), LayerGrantKind::Asset)
        .await
        .unwrap();
    assert_eq!(
        service.observe_use(&intent).await.unwrap().evidence,
        UseEvidence::ProtectedGrantOnly,
    );
    assert_eq!(audit.layer(id(buyer), id(future.term_id)).await, Ok(unknown()));
    grants
        .record(id(buyer), id(chapter), id(version), LayerGrantKind::Chapter)
        .await
        .unwrap();
    assert_eq!(
        service.observe_use(&intent).await.unwrap().evidence,
        UseEvidence::ChapterTextGrantUnconfirmed,
    );
    grants
        .confirm_rendered(id(buyer), id(chapter), id(version))
        .await
        .unwrap();
    assert_eq!(
        service.observe_use(&intent).await.unwrap().evidence,
        UseEvidence::BrowserRenderReported,
    );
    assert_eq!(audit.layer(id(buyer), id(future.term_id)).await, Ok(unknown()));

    sqlx::query(
        "INSERT INTO commerce_layer_term_revocations(term_id,reason,reversal_reference)
         VALUES($1,'legal_block','d06:use-audit-layer-revoke')",
    )
    .bind(current.term_id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        service.observe_use(&intent).await,
        Ok(UseObservation {
            evidence: UseEvidence::BrowserRenderReported,
            access_revoked: true,
        }),
    );
    assert_eq!(
        service.preflight(&intent).await,
        Err(RefundGateError::RevocationPresent),
    );
    assert_eq!(audit.layer(id(buyer), id(future.term_id)).await, Ok(unknown()));
    assert_eq!(no_refund_money(&pool, buyer).await, initial_money);
    pool.close().await;
}
