//! S031: DB-enforced paid-term accounting linkage and exact term duration.
use super::*;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_05_pg_paid_term_requires_matching_wallet_and_command_evidence() {
    let (pool, repo, owner, buyer, book) = setup("wallet-evidence").await;
    let (layer, version, published) = offer(&repo, owner, book, 275, false).await;
    let first_command = cmd(buyer, layer, published, version, 275, 'L');
    let paid = repo.buy(&first_command).await.unwrap();
    assert_eq!(paid.balance_after_minor, 1225);

    // Database writers must not mint 31-day terms even if a caller bypasses
    // the Rust service. NULL expiration also cannot pass a SQL CHECK as UNKNOWN.
    let invalid_duration = sqlx::query(
        "INSERT INTO commerce_layer_terms
         (user_id,layer_id,offer_id,base_version_id,term_kind,amount_minor,
          access_origin_version_id,started_at,expires_at)
         VALUES ($1,$2,$3,$4,'prepaid_30d',275,$4,now(),now()+INTERVAL '31 days')",
    )
    .bind(buyer)
    .bind(layer)
    .bind(published)
    .bind(version)
    .execute(&pool)
    .await;
    assert!(
        invalid_duration.is_err(),
        "duration must be exactly 30 days"
    );
    let missing_expiry = sqlx::query(
        "INSERT INTO commerce_layer_terms
         (user_id,layer_id,offer_id,base_version_id,term_kind,amount_minor,
          access_origin_version_id,started_at,expires_at)
         VALUES ($1,$2,$3,$4,'prepaid_30d',275,$4,now(),NULL)",
    )
    .bind(buyer)
    .bind(layer)
    .bind(published)
    .bind(version)
    .execute(&pool)
    .await;
    assert!(
        missing_expiry.is_err(),
        "paid 30-day terms require an expiry"
    );

    // Once paid, even an app-role SQL UPDATE cannot rewrite or unlink the
    // original command. The mutable NULL->term linkage is a one-time step.
    let change_price = sqlx::query(
        "UPDATE commerce_layer_commands SET expected_price_minor=276
         WHERE user_id=$1 AND command_key=$2",
    )
    .bind(buyer)
    .bind(first_command.key.as_str())
    .execute(&pool)
    .await;
    assert!(
        change_price.is_err(),
        "completed command price is immutable"
    );
    let unlink = sqlx::query(
        "UPDATE commerce_layer_commands SET term_id=NULL
         WHERE user_id=$1 AND command_key=$2",
    )
    .bind(buyer)
    .bind(first_command.key.as_str())
    .execute(&pool)
    .await;
    assert!(unlink.is_err(), "completed term cannot lose its command");

    // A fabricated term with full B-10 share snapshots AND a plausible
    // completed idempotency command still cannot commit without the exact
    // immutable reader wallet purchase journal row.
    let mut tx = pool.begin().await.unwrap();
    let fake_term: i64 = sqlx::query_scalar(
        "INSERT INTO commerce_layer_terms
         (user_id,layer_id,offer_id,base_version_id,term_kind,amount_minor,
          access_origin_version_id,started_at,expires_at)
         VALUES ($1,$2,$3,$4,'prepaid_30d',275,$4,now(),now()+INTERVAL '30 days')
         RETURNING id",
    )
    .bind(buyer)
    .bind(layer)
    .bind(published)
    .bind(version)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO commerce_layer_term_share_snapshots(term_id,book_id,share_revision)
         SELECT $1,book_id,share_revision
         FROM commerce_layer_term_share_snapshots WHERE term_id=$2",
    )
    .bind(fake_term)
    .bind(paid.term_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO commerce_layer_term_share_lines(term_id,book_id,user_id,share_percent)
         SELECT $1,book_id,user_id,share_percent
         FROM commerce_layer_term_share_lines WHERE term_id=$2",
    )
    .bind(fake_term)
    .bind(paid.term_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    let fake_command = cmd(buyer, layer, published, version, 275, 'M');
    sqlx::query(
        "INSERT INTO commerce_layer_commands
         (user_id,command_key,layer_id,expected_offer_id,expected_version_id,
          expected_price_minor,term_id)
         VALUES ($1,$2,$3,$4,$5,275,$6)",
    )
    .bind(buyer)
    .bind(fake_command.key.as_str())
    .bind(layer)
    .bind(published)
    .bind(version)
    .bind(fake_term)
    .execute(&mut *tx)
    .await
    .unwrap();
    let error = tx
        .commit()
        .await
        .expect_err("paid term without wallet evidence must roll back");
    assert!(
        error
            .to_string()
            .contains("paid Layer term lacks matching wallet and command evidence"),
        "expect exact DB wallet evidence guard, not an unrelated share failure: {error}",
    );

    let (terms, journals, balance): (i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM commerce_layer_terms
            WHERE user_id=$1 AND layer_id=$2),
           (SELECT COUNT(*) FROM reader_wallet_entries
            WHERE user_id=$1 AND kind='layer_purchase'),
           (SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1)",
    )
    .bind(buyer)
    .bind(layer)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((terms, journals, balance), (1, 1, 1225));
    assert_eq!(
        repo.buy(&first_command).await.unwrap().term_id,
        paid.term_id,
        "original purchase remains replayable after rejected raw inserts"
    );
    pool.close().await;
}
