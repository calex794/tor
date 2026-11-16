//! S031/R1: a disconnected checkout pool must not create money or rights.
use super::*;

async fn financial_snapshot(pool: &PgPool, buyer: i64) -> (i64, i64, i64, i64) {
    sqlx::query_as(
        "SELECT
            (SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1),
            (SELECT count(*) FROM commerce_layer_terms WHERE user_id=$1),
            (SELECT count(*) FROM commerce_layer_commands WHERE user_id=$1),
            (SELECT count(*) FROM reader_wallet_entries
             WHERE user_id=$1 AND kind='layer_purchase')",
    )
    .bind(buyer)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_checkout_closed_pool_fails_without_money_or_rights_then_retries_once() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG test URL");
    assert!(
        url.ends_with("/tor_ci"),
        "never run against a live database"
    );
    let (pool, disconnected, owner, buyer, book) = setup("checkout-pool-outage").await;
    let (layer, version, offer_id) = offer(&disconnected, owner, book, 250, false).await;
    let buy = cmd(buyer, layer, offer_id, version, 250, 'Z');

    // This independent connection is only a witness. Never shut down the PG
    // server or mutate another integration test's shared disposable database.
    let witness = PgPool::connect(&url).await.unwrap();
    let before = financial_snapshot(&witness, buyer).await;
    assert_eq!(before, (1500, 0, 0, 0));
    pool.close().await;

    assert_eq!(
        disconnected.buy(&buy).await,
        Err(LayerError::Unavailable),
        "an unavailable connection cannot be treated as a successful purchase"
    );
    assert_eq!(financial_snapshot(&witness, buyer).await, before);

    // Reconnect with the exact same command key. The initial failed request
    // created neither a debit nor an entitlement; a later retry may commit once.
    let recovered = PgLayers::new(witness.clone());
    let receipt = recovered.buy(&buy).await.unwrap();
    assert!(!receipt.replayed);
    assert_eq!(receipt.balance_after_minor, 1250);
    let committed = financial_snapshot(&witness, buyer).await;
    assert_eq!(committed, (1250, 1, 1, 1));
    assert!(recovered.can_read_book(id(buyer), id(book)).await.unwrap());

    let retry = recovered.buy(&buy).await.unwrap();
    assert!(retry.replayed);
    assert_eq!(retry.term_id, receipt.term_id);
    assert_eq!(retry.balance_after_minor, receipt.balance_after_minor);
    assert_eq!(financial_snapshot(&witness, buyer).await, committed);
    witness.close().await;
}
