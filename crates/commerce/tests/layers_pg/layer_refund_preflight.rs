//! D-06: no policy or trusted access-exposure facts means no refund writes.
use super::*;
use tor_commerce::{
    repositories::refund_preflight::PgLayerRefundPreflight,
    services::refunds::{LayerRefundIntent, LayerRefundService, RefundGateError},
};

fn refund_intent(actor: i64, term: i64, suffix: char) -> LayerRefundIntent {
    let key = IdempotencyKey::parse(&format!("{}8", suffix.to_string().repeat(42))).unwrap();
    LayerRefundIntent::new(id(actor), id(term), "Платная глава недоступна", key).unwrap()
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_pg_layer_refund_policy_missing_fails_closed_without_wallet_or_rights_changes() {
    let (pool, layers, owner, buyer, book) = setup("d06-policy-unavailable").await;
    let (layer, version, current_offer) = offer(&layers, owner, book, 275, false).await;
    let purchase = cmd(buyer, layer, current_offer, version, 275, 'K');
    let receipt = layers.buy(&purchase).await.unwrap();
    // Existing and freshly purchased terms must never silently inherit an
    // unapproved version or an invented deadline.
    let no_policy: bool = sqlx::query_scalar(
        "SELECT refund_policy_version IS NULL AND refund_deadline_at IS NULL
         FROM commerce_layer_terms WHERE id=$1",
    )
    .bind(receipt.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(no_policy, "checkout must not silently invent refund terms");

    // A later change to terms is forbidden by the original paid-history
    // immutable trigger, even if the caller supplies both fields together.
    let retroactive = sqlx::query(
        "UPDATE commerce_layer_terms
         SET refund_policy_version='ci-unapproved',
             refund_deadline_at=purchased_at+INTERVAL '7 days'
         WHERE id=$1",
    )
    .bind(receipt.term_id)
    .execute(&pool)
    .await;
    assert!(retroactive.is_err(), "never retrofit paid refund terms");

    // The CHECK must reject either half of a forged policy snapshot before
    // the deferred ledger-evidence trigger can see the fabricated term.
    let only_version = sqlx::query(
        "INSERT INTO commerce_layer_terms
         (user_id,layer_id,offer_id,base_version_id,access_origin_version_id,
          term_kind,amount_minor,started_at,expires_at,refund_policy_version)
         SELECT user_id,layer_id,offer_id,base_version_id,access_origin_version_id,
                term_kind,amount_minor,started_at,expires_at,'ci-unapproved'
         FROM commerce_layer_terms WHERE id=$1",
    )
    .bind(receipt.term_id)
    .execute(&pool)
    .await
    .expect_err("a policy version without its purchase deadline must fail");
    assert!(
        only_version
            .to_string()
            .contains("d06_term_refund_snapshot_complete"),
        "must reject due to the new snapshot CHECK: {only_version}"
    );
    let only_deadline = sqlx::query(
        "INSERT INTO commerce_layer_terms
         (user_id,layer_id,offer_id,base_version_id,access_origin_version_id,
          term_kind,amount_minor,started_at,expires_at,refund_deadline_at)
         SELECT user_id,layer_id,offer_id,base_version_id,access_origin_version_id,
                term_kind,amount_minor,started_at,expires_at,purchased_at
         FROM commerce_layer_terms WHERE id=$1",
    )
    .bind(receipt.term_id)
    .execute(&pool)
    .await
    .expect_err("a refund deadline without its policy version must fail");
    assert!(
        only_deadline
            .to_string()
            .contains("d06_term_refund_snapshot_complete"),
        "must reject due to the new snapshot CHECK: {only_deadline}"
    );
    let service = LayerRefundService::new(PgLayerRefundPreflight::new(pool.clone()));
    let intent = refund_intent(buyer, receipt.term_id, 'L');

    // A real paid term is NOT evidence of a refund policy/version/deadline.
    assert_eq!(
        service.preflight(&intent).await,
        Err(RefundGateError::PolicyUnavailable)
    );
    // Neither another actor nor a fabricated term may reveal a purchase.
    assert_eq!(
        service
            .preflight(&refund_intent(owner, receipt.term_id, 'M'))
            .await,
        Err(RefundGateError::NotFound)
    );
    assert_eq!(
        service
            .preflight(&refund_intent(buyer, i64::MAX, 'N'))
            .await,
        Err(RefundGateError::NotFound)
    );

    let before: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
          (SELECT COUNT(*) FROM commerce_layer_terms WHERE layer_id=$1),
          (SELECT COUNT(*) FROM reader_wallet_entries
            WHERE user_id=$2 AND kind='layer_purchase'),
          (SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$2),
          (SELECT COUNT(*) FROM commerce_layer_term_revocations WHERE term_id=$3)",
    )
    .bind(layer)
    .bind(buyer)
    .bind(receipt.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(before, (1, 1, 1225, 0));
    assert!(layers.can_read_book(id(buyer), id(book)).await.unwrap());
    assert_eq!(
        layers.buy(&purchase).await.unwrap().term_id,
        receipt.term_id
    );
    assert_eq!(
        service.preflight(&intent).await,
        Err(RefundGateError::PolicyUnavailable)
    );
    let after: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
          (SELECT COUNT(*) FROM commerce_layer_terms WHERE layer_id=$1),
          (SELECT COUNT(*) FROM reader_wallet_entries
            WHERE user_id=$2 AND kind='layer_purchase'),
          (SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$2),
          (SELECT COUNT(*) FROM commerce_layer_term_revocations WHERE term_id=$3)",
    )
    .bind(layer)
    .bind(buyer)
    .bind(receipt.term_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(after, before);
    assert!(layers.can_read_book(id(buyer), id(book)).await.unwrap());

    // A historical access revocation does not prove a refund or authorize a
    // second disposition. We must not synthesize a "refunded" money status.
    sqlx::query(
        "INSERT INTO commerce_layer_term_revocations
            (term_id,reason,reversal_reference)
         VALUES($1,'legal_block','d06-test-no-reversal')",
    )
    .bind(receipt.term_id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        service.preflight(&intent).await,
        Err(RefundGateError::RevocationPresent)
    );
    let wallet_balance: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(buyer)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(wallet_balance, 1225, "revocation is not a refund posting");
    pool.close().await;
}
