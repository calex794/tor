//! D/S031 R1: a PostgreSQL error raised at COMMIT must roll back the entire
//! financial purchase, not just the Layer entitlement or the receipt.
use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_deferred_commit_rejection_preserves_balance_without_paid_rights() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG test URL");
    assert_eq!(
        url, "postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_ci",
        "financial fault injection may only use disposable CI PostgreSQL"
    );
    let migrator_url = std::env::var("MIGRATOR_URL").expect("disposable PG migrator");
    assert_eq!(
        migrator_url, "postgresql://tor_migrator:tor-ci-migrate@127.0.0.1:5432/tor_ci",
        "never install fault injection on production"
    );

    let (pool, repo, owner, buyer, book) = setup("commit-rollback").await;
    let (layer, version, offer_id) = offer(&repo, owner, book, 250, false).await;
    let buy = cmd(buyer, layer, offer_id, version, 250, 'M');
    let migrator = PgPool::connect(&migrator_url).await.unwrap();

    // An initially-deferred constraint trigger runs at COMMIT after the
    // command, wallet debit, term, revenue snapshots and ledger are staged.
    // A single-quoted function body avoids SQLx's dollar-quote handling.
    sqlx::query(
        "CREATE FUNCTION ci_s031_reject_term_commit() RETURNS trigger AS
         'BEGIN
            RAISE EXCEPTION ''S031 injected COMMIT rollback''
              USING ERRCODE = ''40001'';
          END'
         LANGUAGE plpgsql",
    )
    .execute(&migrator)
    .await
    .unwrap();
    sqlx::query(
        "CREATE CONSTRAINT TRIGGER ci_s031_deferred_term_commit
         AFTER INSERT ON commerce_layer_terms
         DEFERRABLE INITIALLY DEFERRED
         FOR EACH ROW
         EXECUTE FUNCTION ci_s031_reject_term_commit()",
    )
    .execute(&migrator)
    .await
    .unwrap();

    assert_eq!(
        repo.buy(&buy).await,
        Err(LayerError::Unavailable),
        "deferred COMMIT rejection must not return a success receipt"
    );

    let failed: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1),
           (SELECT count(*) FROM commerce_layer_terms WHERE user_id=$1),
           (SELECT count(*) FROM commerce_layer_commands WHERE user_id=$1),
           (SELECT count(*) FROM reader_wallet_entries
            WHERE user_id=$1 AND kind='layer_purchase')",
    )
    .bind(buyer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        failed, (1500, 0, 0, 0),
        "failure at COMMIT must atomically restore wallet, command, term and ledger"
    );
    assert!(
        !repo.can_read_book(id(buyer), id(book)).await.unwrap(),
        "uncommitted Layer entitlement cannot grant access"
    );

    // Removing only the CI-injected trigger makes the original key retryable:
    // the rejected transaction cannot poison the idempotency record.
    sqlx::query("DROP TRIGGER ci_s031_deferred_term_commit ON commerce_layer_terms")
        .execute(&migrator)
        .await
        .unwrap();
    sqlx::query("DROP FUNCTION ci_s031_reject_term_commit()")
        .execute(&migrator)
        .await
        .unwrap();
    migrator.close().await;

    let accepted = repo.buy(&buy).await.unwrap();
    assert!(!accepted.replayed);
    assert_eq!(accepted.balance_after_minor, 1250);
    let replayed = repo.buy(&buy).await.unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.term_id, accepted.term_id);
    assert_eq!(replayed.balance_after_minor, 1250);
    let recovered: (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1),
           (SELECT count(*) FROM commerce_layer_terms WHERE user_id=$1),
           (SELECT count(*) FROM commerce_layer_commands WHERE user_id=$1),
           (SELECT count(*) FROM reader_wallet_entries
            WHERE user_id=$1 AND kind='layer_purchase')",
    )
    .bind(buyer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(recovered, (1250, 1, 1, 1));
    assert!(repo.can_read_book(id(buyer), id(book)).await.unwrap());
    pool.close().await;
}
