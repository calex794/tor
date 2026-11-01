//! D-06 financial-safety acceptance tests against disposable PostgreSQL 18.
use super::*;
use tor_commerce::repositories::{
    direct_exposure::{DirectGrantKind, PgDirectFirstExposure},
    direct_swaps::{PgDirectSwaps, SwapError},
};

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_swap_refuses_target_already_purchased_without_revoking_source() {
    let (pool, intent, source_chapter, source_version) =
        super::d06_swap::swap_fixture("d06_swap_already_owned", 450).await;
    let buyer_checkout = PgCheckout::new(pool.clone()).with_creator_posting(true);
    let owned = buyer_checkout
        .checkout_once(&command(
            intent.buyer_id,
            intent.target_chapter_id,
            intent.target_version_id,
            450,
            'W',
        ))
        .await
        .unwrap();
    let swap_key = IdempotencyKey::parse(&format!("{}8", "H".repeat(42))).unwrap();
    assert_eq!(
        PgDirectSwaps::new(pool.clone())
            .swap_once(intent, &swap_key)
            .await,
        Err(SwapError::AlreadyOwned),
    );
    let (wallet, swapped, revoked, owned_count): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT balance_minor,
           (SELECT COUNT(*) FROM finance_direct_swap_postings
            WHERE source_purchase_id=$2),
           (SELECT COUNT(*) FROM chapter_purchase_revocations WHERE purchase_id=$2),
           (SELECT COUNT(*) FROM chapter_purchase_facts
            WHERE user_id=$1 AND chapter_id=$3)
         FROM reader_wallet_accounts WHERE user_id=$1",
    )
    .bind(intent.buyer_id)
    .bind(intent.source_purchase_id)
    .bind(intent.target_chapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((wallet, swapped, revoked, owned_count), (250, 0, 0, 1));
    let readable: bool =
        sqlx::query_scalar("SELECT tor_purchase_version_readable($1,$2,$3,$4)")
            .bind(intent.buyer_id)
            .bind(intent.source_purchase_id)
            .bind(source_chapter)
            .bind(source_version)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(readable, "an ordinary second purchase must not revoke source");
    assert_eq!(owned.balance_after.get(), 250);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_swap_refuses_previously_opened_purchase_without_credit() {
    let (pool, intent, source_chapter, source_version) =
        super::d06_swap::swap_fixture("d06_swap_already_opened", 450).await;
    let exposure = PgDirectFirstExposure::new(pool.clone());
    exposure
        .record(
            PositiveId::new(intent.buyer_id).unwrap(),
            PositiveId::new(source_chapter).unwrap(),
            PositiveId::new(source_version).unwrap(),
            Some(PositiveId::new(intent.source_purchase_id).unwrap()),
            DirectGrantKind::Chapter,
        )
        .await
        .unwrap();
    let swap_key = IdempotencyKey::parse(&format!("{}8", "I".repeat(42))).unwrap();
    assert_eq!(
        PgDirectSwaps::new(pool.clone())
            .swap_once(intent, &swap_key)
            .await,
        Err(SwapError::Opened),
    );
    let (wallet, postings, credits, revoked): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT balance_minor,
           (SELECT COUNT(*) FROM finance_direct_swap_postings
            WHERE source_purchase_id=$2),
           (SELECT COUNT(*) FROM reader_wallet_entries
            WHERE purchase_id=$2 AND kind='chapter_swap_credit'),
           (SELECT COUNT(*) FROM chapter_purchase_revocations
            WHERE purchase_id=$2)
         FROM reader_wallet_accounts WHERE user_id=$1",
    )
    .bind(intent.buyer_id)
    .bind(intent.source_purchase_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((wallet, postings, credits, revoked), (700, 0, 0, 0));
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_swap_reconciles_wallet_platform_and_creator_balances() {
    let (pool, intent, _, _) =
        super::d06_swap::swap_fixture("d06_swap_finance_conservation", 450).await;
    let swap_key = IdempotencyKey::parse(&format!("{}8", "J".repeat(42))).unwrap();
    let receipt = PgDirectSwaps::new(pool.clone())
        .swap_once(intent, &swap_key)
        .await
        .unwrap();
    let source: (i64, i64, i64) = sqlx::query_as(
        "SELECT gross_minor,platform_fee_minor,creator_pool_minor
         FROM finance_chapter_postings WHERE purchase_id=$1",
    )
    .bind(intent.source_purchase_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let target: (i64, i64, i64) = sqlx::query_as(
        "SELECT gross_minor,platform_fee_minor,creator_pool_minor
         FROM finance_chapter_postings WHERE purchase_id=$1",
    )
    .bind(receipt.new_purchase)
    .fetch_one(&pool)
    .await
    .unwrap();
    let reversal: (i64, i64) = sqlx::query_as(
        "SELECT platform_reversal_minor,creator_reversal_minor
         FROM finance_direct_swap_postings WHERE source_purchase_id=$1",
    )
    .bind(intent.source_purchase_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let source_creator: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(amount_minor),0)::BIGINT
         FROM finance_creator_earnings WHERE purchase_id=$1",
    )
    .bind(intent.source_purchase_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let target_creator: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(amount_minor),0)::BIGINT
         FROM finance_creator_earnings WHERE purchase_id=$1",
    )
    .bind(receipt.new_purchase)
    .fetch_one(&pool)
    .await
    .unwrap();
    let reversed_creator: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(amount_minor),0)::BIGINT
         FROM finance_direct_swap_creator_reversals WHERE source_purchase_id=$1",
    )
    .bind(intent.source_purchase_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let source_platform: i64 = sqlx::query_scalar(
        "SELECT amount_minor FROM finance_platform_credits WHERE purchase_id=$1",
    )
    .bind(intent.source_purchase_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let target_platform: i64 = sqlx::query_scalar(
        "SELECT amount_minor FROM finance_platform_credits WHERE purchase_id=$1",
    )
    .bind(receipt.new_purchase)
    .fetch_one(&pool)
    .await
    .unwrap();
    let wallet: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(intent.buyer_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    assert_eq!(source.0, source.1 + source.2);
    assert_eq!(target.0, target.1 + target.2);
    assert_eq!((source.1, source.2), (source_platform, source_creator));
    assert_eq!((target.1, target.2), (target_platform, target_creator));
    assert_eq!(reversal, (source.1, source.2));
    assert_eq!(reversed_creator, source_creator);
    assert_eq!(
        (target.1 - reversal.0) + (target.2 - reversal.1),
        target.0 - source.0,
        "net platform and creator claims must equal the reader's price difference",
    );
    assert_eq!(1000 - wallet, target.0, "the reader pays only for the final chapter");
    assert_eq!(receipt.balance_after, wallet);
    pool.close().await;
}
