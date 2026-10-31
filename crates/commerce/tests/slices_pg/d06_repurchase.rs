use super::d06_refund_approval::{d06_approval_cleanup, d06_approval_fixture, issue_refund_step_up};
use super::*;
use tor_commerce::{
    domain::batch_checkout::{BatchChapterOffer, BatchCheckoutError, BatchPurchaseCommand},
    repositories::{
        direct_exposure::{DirectGrantKind, PgDirectFirstExposure},
        refund_approvals::PgDirectRefundApprovals,
    },
};
use tor_contracts::paid_delivery::DeliveryError;

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_repurchase_after_approved_refund_keeps_full_old_money_history() {
    let f = d06_approval_fixture("d06_repurchase_once", 86400).await;
    let (session_digest, step_up_token) = issue_refund_step_up(&f, "approved").await;
    let approval = PgDirectRefundApprovals::new(f.pool.clone())
        .approve_once(f.moderator, &session_digest, &step_up_token, f.request, 1, "approved")
        .await
        .unwrap();
    assert_eq!(approval.credited_minor, 300);

    let disabled = PgCheckout::new(f.pool.clone());
    let retry_key = command(f.buyer, f.chapter, f.version, 300, 'R');
    assert_eq!(
        disabled.checkout_once(&retry_key).await,
        Err(CheckoutError::Unavailable),
        "wallet-only creator-free repurchase must fail closed"
    );
    let before: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(f.buyer)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(before, 1000);
    let enabled = PgCheckout::new(f.pool.clone()).with_creator_posting(true);
    let repurchased = enabled.checkout_once(&retry_key).await.unwrap();
    assert_ne!(repurchased.purchase_id.get(), f.purchase);
    assert_eq!(repurchased.amount.get(), 300);
    assert_eq!(repurchased.balance_after.get(), 700);

    let (live, historical, reversals, audit): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
                (SELECT purchase_id FROM library_purchased_chapters
                 WHERE user_id=$1 AND chapter_id=$2),
                (SELECT count(*) FROM chapter_purchase_facts
                 WHERE user_id=$1 AND chapter_id=$2),
                (SELECT count(*) FROM finance_direct_refund_postings
                 WHERE purchase_id=$3),
                (SELECT count(*) FROM library_purchase_reacquisitions
                 WHERE old_purchase_id=$3 AND new_purchase_id=$4
                   AND user_id=$1 AND chapter_id=$2)",
    )
    .bind(f.buyer)
    .bind(f.chapter)
    .bind(f.purchase)
    .bind(repurchased.purchase_id.get())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(live, repurchased.purchase_id.get());
    assert_eq!((historical, reversals, audit), (2, 1, 1));
    let new_finance: (i64, i64, i64) = sqlx::query_as(
        "SELECT gross_minor,platform_fee_minor,creator_pool_minor
         FROM finance_chapter_postings WHERE purchase_id=$1",
    )
    .bind(repurchased.purchase_id.get())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(new_finance, (300, 60, 240));

    // Old refund/revocation stays enforceable. Only the new purchase grants
    // access, with a distinct purchase id, snapshot and creator credit.
    let old_readable: bool =
        sqlx::query_scalar("SELECT tor_purchase_version_readable($1,$2,$3,$4)")
            .bind(f.buyer)
            .bind(f.purchase)
            .bind(f.chapter)
            .bind(f.version)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    let new_readable: bool =
        sqlx::query_scalar("SELECT tor_purchase_version_readable($1,$2,$3,$4)")
            .bind(f.buyer)
            .bind(repurchased.purchase_id.get())
            .bind(f.chapter)
            .bind(f.version)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert!(!old_readable);
    assert!(new_readable);

    let old_grant = PgDirectFirstExposure::new(f.pool.clone())
        .record(
            PositiveId::new(f.buyer).unwrap(),
            PositiveId::new(f.chapter).unwrap(),
            PositiveId::new(f.version).unwrap(),
            Some(PositiveId::new(f.purchase).unwrap()),
            DirectGrantKind::Chapter,
        )
        .await;
    assert_eq!(old_grant, Err(DeliveryError::Denied));

    let original_replay = enabled
        .checkout_once(&command(f.buyer, f.chapter, f.version, 300, 'Q'))
        .await
        .unwrap();
    assert!(original_replay.replayed);
    assert_eq!(original_replay.purchase_id.get(), f.purchase);
    let replay = enabled.checkout_once(&retry_key).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.purchase_id, repurchased.purchase_id);
    assert_eq!(
        enabled
            .checkout_once(&command(f.buyer, f.chapter, f.version, 300, 'S'))
            .await,
        Err(CheckoutError::AlreadyOwned)
    );

    // There is no way for direct SQL to undo or fake a trusted pointer.
    assert!(sqlx::query(
        "UPDATE library_purchased_chapters SET purchase_id=$3
         WHERE user_id=$1 AND chapter_id=$2",
    )
    .bind(f.buyer)
    .bind(f.chapter)
    .bind(f.purchase)
    .execute(&f.pool)
    .await
    .is_err());
    assert!(
        sqlx::query("DELETE FROM library_purchase_reacquisitions WHERE new_purchase_id=$1",)
            .bind(repurchased.purchase_id.get())
            .execute(&f.pool)
            .await
            .is_err()
    );
    d06_approval_cleanup(f).await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_repurchase_two_distinct_keys_only_one_new_wallet_debit() {
    let f = d06_approval_fixture("d06_repurchase_race", 86400).await;
    let (session_digest, step_up_token) = issue_refund_step_up(&f, "approved").await;
    PgDirectRefundApprovals::new(f.pool.clone())
        .approve_once(f.moderator, &session_digest, &step_up_token, f.request, 1, "approved")
        .await
        .unwrap();
    let checkout = PgCheckout::new(f.pool.clone()).with_creator_posting(true);
    let command_left = command(f.buyer, f.chapter, f.version, 300, 'R');
    let command_right = command(f.buyer, f.chapter, f.version, 300, 'S');
    let (left, right) = tokio::join!(
        checkout.checkout_once(&command_left),
        checkout.checkout_once(&command_right)
    );
    let success = [left, right]
        .into_iter()
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    assert_eq!(success.len(), 1);
    let money: (i64, i64, i64) = sqlx::query_as(
        "SELECT a.balance_minor,
                (SELECT count(*) FROM library_purchase_reacquisitions
                 WHERE old_purchase_id=$2),
                (SELECT count(*) FROM reader_wallet_entries e
                 WHERE e.user_id=$1 AND e.kind='chapter_purchase')
         FROM reader_wallet_accounts a WHERE a.user_id=$1",
    )
    .bind(f.buyer)
    .bind(f.purchase)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(money, (700, 1, 2));
    d06_approval_cleanup(f).await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_batch_can_buy_refunded_and_new_chapter_atomically() {
    let f = d06_approval_fixture("d06_repurchase_batch", 86400).await;
    let (session_digest, step_up_token) = issue_refund_step_up(&f, "approved").await;
    PgDirectRefundApprovals::new(f.pool.clone())
        .approve_once(f.moderator, &session_digest, &step_up_token, f.request, 1, "approved")
        .await
        .unwrap();
    let book: i64 = sqlx::query_scalar("SELECT book_id FROM chapters WHERE id=$1")
        .bind(f.chapter)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let (second, v2) = chapter_with_offer(&f.pool, book, 2, 101).await;
    let disabled = PgCheckout::new(f.pool.clone());
    let preview = disabled
        .selectable_batch_offers(f.buyer, book)
        .await
        .unwrap();
    assert_eq!(
        preview.len(),
        1,
        "refunded chapter hidden while D-08 disabled"
    );
    let enabled = PgCheckout::new(f.pool.clone()).with_creator_posting(true);
    let preview = enabled
        .selectable_batch_offers(f.buyer, book)
        .await
        .unwrap();
    assert_eq!(preview.len(), 2);
    let offer = |chapter, version, price| BatchChapterOffer {
        edition: ChapterVersionRef {
            chapter_id: PositiveId::new(chapter).unwrap(),
            version_id: PositiveId::new(version).unwrap(),
        },
        expected_price: MoneyMinor::new(price).unwrap(),
    };
    let cmd = BatchPurchaseCommand::new(
        PositiveId::new(f.buyer).unwrap(),
        WalletId::new(f.buyer).unwrap(),
        vec![offer(f.chapter, f.version, 300), offer(second, v2, 101)],
        IdempotencyKey::parse(&format!("{}8", "B".repeat(42))).unwrap(),
    )
    .unwrap();
    assert_eq!(
        disabled.checkout_batch_once(&cmd).await,
        Err(BatchCheckoutError::Checkout(CheckoutError::Unavailable))
    );
    let result = enabled.checkout_batch_once(&cmd).await.unwrap();
    assert_eq!(result.purchases.len(), 2);
    assert_eq!(result.total.get(), 401);
    assert_eq!(result.balance_after.get(), 599);
    let audit: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM library_purchase_reacquisitions
         WHERE old_purchase_id=$1",
    )
    .bind(f.purchase)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(audit, 1);
    let replay = enabled.checkout_batch_once(&cmd).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(
        replay.purchases[0].purchase_id,
        result.purchases[0].purchase_id
    );
    d06_approval_cleanup(f).await;
}
