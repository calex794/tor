//! D/S031: a committed Layer purchase survives lost response and independent retries.
use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_lost_checkout_ack_replays_across_fresh_pools_without_another_debit() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG test URL");
    assert!(
        url.ends_with("/tor_ci"),
        "never run against a live database"
    );
    let (pool, original_node, owner, buyer, book) = setup("lost-ack").await;
    let (layer, version, offer_id) = offer(&original_node, owner, book, 250, false).await;
    let buy = cmd(buyer, layer, offer_id, version, 250, 'L');

    // The database transaction commits, but imagine the client connection
    // dropping before any HTTP acknowledgement reaches the buyer.
    let committed = original_node.buy(&buy).await.unwrap();
    assert!(!committed.replayed);
    let committed_term_id = committed.term_id;
    drop(committed);
    drop(original_node);
    pool.close().await;

    // The reconnecting clients have fresh, independent connection pools.
    let first_pool = PgPool::connect(&url).await.unwrap();
    let second_pool = PgPool::connect(&url).await.unwrap();
    let first = PgLayers::new(first_pool.clone());
    let second = PgLayers::new(second_pool.clone());

    let (first_retry, second_retry) = tokio::join!(first.buy(&buy), second.buy(&buy));
    for receipt in [first_retry.unwrap(), second_retry.unwrap()] {
        assert!(
            receipt.replayed,
            "a committed key must never purchase twice"
        );
        assert_eq!(receipt.term_id, committed_term_id);
        assert_eq!(receipt.balance_after_minor, 1250);
    }

    // A changed payload with the original idempotency key is a conflict, even
    // after a new service instance and while the receipt is durable.
    assert_eq!(
        first
            .buy(&cmd(buyer, layer, offer_id, version, 251, 'L'))
            .await,
        Err(LayerError::ReplayConflict)
    );

    let exact_state: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1),
           (SELECT count(*) FROM commerce_layer_terms WHERE user_id=$1),
           (SELECT count(*) FROM commerce_layer_commands WHERE user_id=$1),
           (SELECT count(*) FROM reader_wallet_entries
            WHERE user_id=$1 AND kind='layer_purchase')",
    )
    .bind(buyer)
    .fetch_one(&first_pool)
    .await
    .unwrap();
    assert_eq!(
        exact_state,
        (1250, 1, 1, 1),
        "replayed lost acknowledgement cannot double-charge or issue duplicate rights"
    );
    assert!(second.can_read_book(id(buyer), id(book)).await.unwrap());

    first_pool.close().await;
    second_pool.close().await;
}
