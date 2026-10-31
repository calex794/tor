use super::*;
use tor_commerce::repositories::{
    direct_swaps::{PgDirectSwaps, SwapError},
    swap_intent::DirectSwapIntent,
};

pub(super) async fn swap_fixture(tag: &str, target_price: i64) -> (PgPool, DirectSwapIntent, i64, i64) {
    let (pool, buyer, source_book, chapter, version) = fixture(tag, 1000, 300).await;
    let owner: i64 = sqlx::query_scalar("SELECT owner_user_id FROM books WHERE id=$1")
        .bind(source_book)
        .fetch_one(&pool)
        .await
        .unwrap();
    let legal = format!("ci:swap:legal:{buyer}");
    let terms = format!("ci:swap:terms:{buyer}");
    sqlx::query(
        "INSERT INTO finance_direct_refund_policy_catalog
          (version,approval_reference,approved_by_user_id,
           approved_at,effective_at,window_seconds)
         VALUES($1,$2,$3,now()-INTERVAL '2 days',
                now()-INTERVAL '1 day',86400)",
    )
    .bind(&legal)
    .bind(format!("ci:swap:approval:{buyer}"))
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();
    let target_book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','Вторая книга того же автора','public',$2) RETURNING id",
    )
    .bind(format!("ci:swap:target:{tag}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    for book in [source_book, target_book] {
        sqlx::query(
            "INSERT INTO finance_revenue_terms
             (book_id,policy_version,platform_fee_bps,
              refund_policy_version,refund_window_seconds)
             VALUES($1,$2,2000,$3,86400)",
        )
        .bind(book)
        .bind(&terms)
        .bind(&legal)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO finance_revenue_term_states(book_id,policy_version)
             VALUES($1,$2)",
        )
        .bind(book)
        .bind(&terms)
        .execute(&pool)
        .await
        .unwrap();
    }
    let source_purchase = PgCheckout::new(pool.clone())
        .with_creator_posting(true)
        .checkout_once(&command(buyer, chapter, version, 300, 'A'))
        .await
        .unwrap()
        .purchase_id
        .get();
    let (target, target_version) = chapter_with_offer(&pool, target_book, 1, target_price).await;
    let intent = DirectSwapIntent {
        buyer_id: buyer,
        source_purchase_id: source_purchase,
        target_chapter_id: target,
        target_version_id: target_version,
        source_price_minor: 300,
        target_price_minor: target_price,
    };
    (pool, intent, chapter, version)
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_swap_atomically_reverses_source_and_posts_target() {
    let (pool, i, source_chapter, source_version) = swap_fixture("d06_swap_higher", 450).await;
    let key = IdempotencyKey::parse(&format!("{}8", "W".repeat(42))).unwrap();
    let swaps = PgDirectSwaps::new(pool.clone());
    let first = swaps.swap_once(i, &key).await.unwrap();
    assert!(!first.replayed);
    assert_eq!(first.delta_minor, 150);
    assert_eq!(first.balance_after, 550);
    let replay = swaps.swap_once(i, &key).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.new_purchase, first.new_purchase);
    let wallet: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(i.buyer_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(wallet, 550);
    let (source_revoked, new_owned, old_credits, new_debits, platform_reverse, creator_reverse): (
        bool,
        bool,
        i64,
        i64,
        i64,
        i64,
    ) = sqlx::query_as(
        "SELECT
           EXISTS(SELECT 1 FROM chapter_purchase_revocations
                  WHERE purchase_id=$1 AND reason='purchase_swap'),
           EXISTS(SELECT 1 FROM library_purchased_chapters
                  WHERE user_id=$2 AND chapter_id=$3 AND purchase_id=$4),
           (SELECT count(*) FROM reader_wallet_entries
            WHERE purchase_id=$1 AND kind='chapter_swap_credit'),
           (SELECT count(*) FROM reader_wallet_entries
            WHERE purchase_id=$4 AND kind='chapter_purchase'),
           (SELECT platform_reversal_minor FROM finance_direct_swap_postings
            WHERE source_purchase_id=$1),
           (SELECT COALESCE(sum(amount_minor),0)::BIGINT
            FROM finance_direct_swap_creator_reversals
            WHERE source_purchase_id=$1)",
    )
    .bind(i.source_purchase_id)
    .bind(i.buyer_id)
    .bind(i.target_chapter_id)
    .bind(first.new_purchase)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(source_revoked && new_owned);
    assert_eq!(
        (old_credits, new_debits, platform_reverse, creator_reverse),
        (1, 1, 60, 240)
    );
    let old_readable: bool =
        sqlx::query_scalar("SELECT tor_purchase_version_readable($1,$2,$3,$4)")
            .bind(i.buyer_id)
            .bind(i.source_purchase_id)
            .bind(source_chapter)
            .bind(source_version)
            .fetch_one(&pool)
            .await
            .unwrap();
    let new_readable: bool =
        sqlx::query_scalar("SELECT tor_purchase_version_readable($1,$2,$3,$4)")
            .bind(i.buyer_id)
            .bind(first.new_purchase)
            .bind(i.target_chapter_id)
            .bind(i.target_version_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!old_readable && new_readable);
    assert_eq!(
        swaps
            .swap_once(
                DirectSwapIntent {
                    target_price_minor: 451,
                    ..i
                },
                &key
            )
            .await,
        Err(SwapError::Conflict)
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_swap_short_wallet_rolls_back_all_postings_and_access() {
    let (pool, i, _, _) = swap_fixture("d06_swap_no_balance", 1100).await;
    let key = IdempotencyKey::parse(&format!("{}8", "X".repeat(42))).unwrap();
    assert_eq!(
        PgDirectSwaps::new(pool.clone()).swap_once(i, &key).await,
        Err(SwapError::Insufficient)
    );
    let (wallet, swap_count, credits, revocations, target_purchases): (i64, i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT balance_minor,
           (SELECT count(*) FROM finance_direct_swap_postings
            WHERE source_purchase_id=$2),
           (SELECT count(*) FROM reader_wallet_entries
            WHERE purchase_id=$2 AND kind='chapter_swap_credit'),
           (SELECT count(*) FROM chapter_purchase_revocations WHERE purchase_id=$2),
           (SELECT count(*) FROM chapter_purchase_facts
            WHERE chapter_id=$3 AND user_id=$1)
         FROM reader_wallet_accounts WHERE user_id=$1",
        )
        .bind(i.buyer_id)
        .bind(i.source_purchase_id)
        .bind(i.target_chapter_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        (wallet, swap_count, credits, revocations, target_purchases),
        (700, 0, 0, 0, 0)
    );
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_swap_two_concurrent_keys_cannot_reverse_source_twice() {
    let (pool, intent, _, _) = swap_fixture("d06_swap_race", 320).await;
    let a = IdempotencyKey::parse(&format!("{}8", "J".repeat(42))).unwrap();
    let b = IdempotencyKey::parse(&format!("{}8", "K".repeat(42))).unwrap();
    let swaps = PgDirectSwaps::new(pool.clone());
    let (left, right) = tokio::join!(swaps.swap_once(intent, &a), swaps.swap_once(intent, &b));
    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let (balance, postings, credits, target_debits): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT a.balance_minor,
           (SELECT count(*) FROM finance_direct_swap_postings
            WHERE source_purchase_id=$2),
           (SELECT count(*) FROM reader_wallet_entries
            WHERE purchase_id=$2 AND kind='chapter_swap_credit'),
           (SELECT count(*) FROM reader_wallet_entries e
            JOIN chapter_purchase_facts p ON p.id=e.purchase_id
            WHERE p.user_id=$1 AND p.chapter_id=$3 AND e.kind='chapter_purchase')
         FROM reader_wallet_accounts a WHERE a.user_id=$1",
    )
    .bind(intent.buyer_id)
    .bind(intent.source_purchase_id)
    .bind(intent.target_chapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((balance, postings, credits, target_debits), (680, 1, 1, 1));
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_swap_two_concurrent_identical_keys_share_one_receipt() {
    let (pool, intent, _, _) = swap_fixture("d06_swap_same_key", 330).await;
    let key = IdempotencyKey::parse(&format!("{}8", "L".repeat(42))).unwrap();
    let swaps = PgDirectSwaps::new(pool.clone());
    let (left, right) = tokio::join!(swaps.swap_once(intent, &key), swaps.swap_once(intent, &key),);
    let left = left.expect("the original exact claim should succeed");
    let right = right.expect("a concurrent identical claim must replay");
    assert_eq!(left.new_purchase, right.new_purchase);
    assert_ne!(left.replayed, right.replayed);
    assert_eq!(left.balance_after, 670);
    assert_eq!(right.balance_after, 670);
    let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM finance_direct_swap_postings
            WHERE source_purchase_id=$1),
           (SELECT COUNT(*) FROM reader_wallet_entries
            WHERE purchase_id=$1 AND kind='chapter_swap_credit'),
           (SELECT COUNT(*) FROM chapter_purchase_facts
            WHERE user_id=$2 AND chapter_id=$3)",
    )
    .bind(intent.source_purchase_id)
    .bind(intent.buyer_id)
    .bind(intent.target_chapter_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(counts, (1, 1, 1), "concurrent replay must not double-post");
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_swap_racing_first_paid_grant_never_refunds_opened_text() {
    use tor_commerce::repositories::direct_exposure::{DirectGrantKind, PgDirectFirstExposure};

    let (pool, intent, source_chapter, source_version) =
        swap_fixture("d06_swap_first_grant_race", 320).await;
    let key = IdempotencyKey::parse(&format!("{}8", "M".repeat(42))).unwrap();
    let swaps = PgDirectSwaps::new(pool.clone());
    let exposure = PgDirectFirstExposure::new(pool.clone());
    let (swap, grant) = tokio::join!(
        swaps.swap_once(intent, &key),
        exposure.record(
            PositiveId::new(intent.buyer_id).unwrap(),
            PositiveId::new(source_chapter).unwrap(),
            PositiveId::new(source_version).unwrap(),
            Some(PositiveId::new(intent.source_purchase_id).unwrap()),
            DirectGrantKind::Chapter,
        ),
    );
    assert_ne!(
        swap.is_ok(),
        grant.is_ok(),
        "exactly one competing operation wins"
    );
    let (postings, exposed, revoked, balance): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
           (SELECT COUNT(*) FROM finance_direct_swap_postings
            WHERE source_purchase_id=$1),
           (SELECT COUNT(*) FROM chapter_purchase_first_exposures
            WHERE purchase_id=$1),
           (SELECT COUNT(*) FROM chapter_purchase_revocations
            WHERE purchase_id=$1 AND reason='purchase_swap'),
           (SELECT balance_minor FROM reader_wallet_accounts
            WHERE user_id=$2)",
    )
    .bind(intent.source_purchase_id)
    .bind(intent.buyer_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    if swap.is_ok() {
        assert!(grant.is_err(), "old grant must be denied after swap");
        assert_eq!((postings, exposed, revoked, balance), (1, 0, 1, 680));
    } else {
        assert_eq!(swap, Err(SwapError::Opened));
        assert!(grant.is_ok(), "the first grant must block swap");
        assert_eq!((postings, exposed, revoked, balance), (0, 1, 0, 700));
    }
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_swap_racing_refund_request_cannot_reverse_source_twice() {
    use tor_commerce::repositories::refund_requests::{
        DirectRefundRequestError, PgDirectRefundRequests,
    };

    let (pool, intent, _, _) = swap_fixture("d06_swap_refund_race", 320).await;
    let swap_key = IdempotencyKey::parse(&format!("{}8", "N".repeat(42))).unwrap();
    let refund_key = IdempotencyKey::parse(&format!("{}8", "O".repeat(42))).unwrap();
    let swaps = PgDirectSwaps::new(pool.clone());
    let refunds = PgDirectRefundRequests::new(pool.clone());
    let (swap, refund) = tokio::join!(
        swaps.swap_once(intent, &swap_key),
        refunds.request_once(
            intent.buyer_id,
            intent.source_purchase_id,
            &refund_key,
            "Swap versus refund request race",
        ),
    );
    assert_ne!(
        swap.is_ok(),
        refund.is_ok(),
        "one exclusive decision must win"
    );
    let (postings, pending_requests, credits, balance): (i64, i64, i64, i64) = sqlx::query_as(
        "SELECT
               (SELECT COUNT(*) FROM finance_direct_swap_postings
                WHERE source_purchase_id=$1),
               (SELECT COUNT(*) FROM finance_direct_refund_requests
                WHERE purchase_id=$1 AND state IN ('pending','processing')),
               (SELECT COUNT(*) FROM reader_wallet_entries
                WHERE purchase_id=$1 AND kind='chapter_swap_credit'),
               (SELECT balance_minor FROM reader_wallet_accounts
                WHERE user_id=$2)",
    )
    .bind(intent.source_purchase_id)
    .bind(intent.buyer_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    if swap.is_ok() {
        assert_eq!(refund, Err(DirectRefundRequestError::Revoked));
        assert_eq!(
            (postings, pending_requests, credits, balance),
            (1, 0, 1, 680)
        );
    } else {
        assert_eq!(swap, Err(SwapError::Refunding));
        assert!(refund.is_ok());
        assert_eq!(
            (postings, pending_requests, credits, balance),
            (0, 1, 0, 700)
        );
    }
    pool.close().await;
}
