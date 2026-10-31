use super::*;
use sqlx::PgPool;
use tor_commerce::repositories::{
    direct_exposure::{DirectGrantKind, PgDirectFirstExposure},
    refund_approvals::{DirectRefundApproveError, PgDirectRefundApprovals},
    refund_requests::PgDirectRefundRequests,
};
use tor_contracts::paid_delivery::{DeliveryError, PositiveId};

struct RefundFixture {
    pool: PgPool,
    buyer: i64,
    moderator: i64,
    chapter: i64,
    version: i64,
    purchase: i64,
    request: i64,
}

async fn d06_approval_fixture(tag: &str, window_seconds: i64) -> RefundFixture {
    let (pool, buyer, book, chapter, version) =
        fixture(tag, 1000, 300).await;
    let moderator: i64 = sqlx::query_scalar(
        "SELECT owner_user_id FROM books WHERE id=$1",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let legal = format!("ci:refund-approval:{buyer}");
    let terms = format!("ci:revenue_approval:{buyer}");
    sqlx::query(
        "INSERT INTO finance_direct_refund_policy_catalog
           (version,approval_reference,approved_by_user_id,
            approved_at,effective_at,window_seconds)
         VALUES($1,$2,$3,now()-INTERVAL '2 days',
                now()-INTERVAL '1 day',$4)",
    )
    .bind(&legal)
    .bind(format!("ci:refund-approved:{buyer}"))
    .bind(moderator)
    .bind(window_seconds)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO finance_revenue_terms
           (book_id,policy_version,platform_fee_bps,
            refund_policy_version,refund_window_seconds)
         VALUES($1,$2,2000,$3,$4)",
    )
    .bind(book)
    .bind(&terms)
    .bind(&legal)
    .bind(window_seconds)
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
    let purchase = PgCheckout::new(pool.clone())
        .with_creator_posting(true)
        .checkout_once(&command(buyer, chapter, version, 300, 'Q'))
        .await
        .unwrap()
        .purchase_id
        .get();
    let key = IdempotencyKey::parse(&format!("{}8", "Z".repeat(42))).unwrap();
    let request = PgDirectRefundRequests::new(pool.clone())
        .request_once(buyer, purchase, &key, "не открывал")
        .await
        .unwrap()
        .request_id;
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'administrator',TRUE)",
    )
    .bind(moderator)
    .execute(&pool)
    .await
    .unwrap();
    RefundFixture {
        pool,
        buyer,
        moderator,
        chapter,
        version,
        purchase,
        request,
    }
}

async fn d06_approval_cleanup(f: RefundFixture) {
    // All tests share one disposable DB in the CI suite: do not leave an
    // extra global administrator behind for Line A's last-admin invariant.
    sqlx::query(
        "UPDATE account_roles SET is_active=FALSE
         WHERE user_id=$1 AND role='administrator'",
    )
    .bind(f.moderator)
    .execute(&f.pool)
    .await
    .unwrap();
    f.pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_refund_approve_atomically_returns_wallet_and_reverses_20_80_split() {
    let f = d06_approval_fixture("d06_refund_approve_ok", 86400).await;
    let refunds = PgDirectRefundApprovals::new(f.pool.clone());
    assert_eq!(
        refunds.approve_once(f.buyer, f.request, 1, "approved").await,
        Err(DirectRefundApproveError::Forbidden)
    );

    let first = refunds
        .approve_once(f.moderator, f.request, 1, "approved")
        .await
        .unwrap();
    assert!(!first.replayed);
    assert_eq!(first.credited_minor, 300);
    assert_eq!(first.revision, 3);
    assert_eq!(first.purchase_id, f.purchase);
    let balance: i64 = sqlx::query_scalar(
        "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1",
    )
    .bind(f.buyer)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(balance, 1000);

    let (buyer_credit, platform_reverse, creator_reverse): (i64, i64, i64) =
        sqlx::query_as(
            "SELECT e.delta_minor, r.platform_reversal_minor,
                    r.creator_reversal_minor
             FROM reader_wallet_entries e
             JOIN finance_direct_refund_postings r ON r.purchase_id=e.purchase_id
             WHERE r.purchase_id=$1 AND e.kind='chapter_refund'",
        )
        .bind(f.purchase)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!((buyer_credit, platform_reverse, creator_reverse), (300, 60, 240));
    let reversed_creator: i64 = sqlx::query_scalar(
        "SELECT SUM(amount_minor) FROM finance_direct_refund_creator_reversals
         WHERE purchase_id=$1",
    )
    .bind(f.purchase)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(reversed_creator, 240);
    let revoked: bool = sqlx::query_scalar(
        "SELECT EXISTS(
           SELECT 1 FROM chapter_purchase_revocations
           WHERE purchase_id=$1 AND reason='refund')",
    )
    .bind(f.purchase)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert!(revoked);

    let replay = refunds
        .approve_once(f.moderator, f.request, 1, "approved")
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.credited_minor, 300);
    assert_eq!(replay.revision, 3);
    assert_eq!(
        refunds
            .approve_once(f.moderator, f.request, 1, "different reason")
            .await,
        Err(DirectRefundApproveError::Conflict)
    );
    let refund_credits: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reader_wallet_entries
         WHERE purchase_id=$1 AND kind='chapter_refund'",
    )
    .bind(f.purchase)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(refund_credits, 1);
    let readable: bool = sqlx::query_scalar(
        "SELECT tor_purchase_version_readable($1,$2,$3,$4)",
    )
    .bind(f.buyer)
    .bind(f.purchase)
    .bind(f.chapter)
    .bind(f.version)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert!(!readable, "approved refund revokes the old paid right");
    d06_approval_cleanup(f).await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_refund_pending_then_first_grant_cannot_approve() {
    let f = d06_approval_fixture("d06_refund_grant_race", 86400).await;
    PgDirectFirstExposure::new(f.pool.clone())
        .record(
            PositiveId::new(f.buyer).unwrap(),
            PositiveId::new(f.chapter).unwrap(),
            PositiveId::new(f.version).unwrap(),
            Some(PositiveId::new(f.purchase).unwrap()),
            DirectGrantKind::Chapter,
        )
        .await
        .unwrap();

    let result = PgDirectRefundApprovals::new(f.pool.clone())
        .approve_once(f.moderator, f.request, 1, "not opened")
        .await;
    assert_eq!(result, Err(DirectRefundApproveError::AlreadyOpened));
    let reverse_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM finance_direct_refund_postings WHERE purchase_id=$1",
    )
    .bind(f.purchase)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(reverse_count, 0);
    let balance: i64 = sqlx::query_scalar(
        "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1",
    )
    .bind(f.buyer)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(balance, 700);
    d06_approval_cleanup(f).await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_refund_pending_then_expired_cannot_approve() {
    let f = d06_approval_fixture("d06_refund_deadline", 1).await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert_eq!(
        PgDirectRefundApprovals::new(f.pool.clone())
            .approve_once(f.moderator, f.request, 1, "too late")
            .await,
        Err(DirectRefundApproveError::WindowExpired)
    );
    let posting_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM finance_direct_refund_postings
         WHERE purchase_id=$1",
    )
    .bind(f.purchase)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(posting_count, 0);
    d06_approval_cleanup(f).await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_refund_approval_and_first_grant_compete_under_purchase_lock() {
    let f = d06_approval_fixture("d06_refund_concurrency", 86400).await;
    let approvals = PgDirectRefundApprovals::new(f.pool.clone());
    let grants = PgDirectFirstExposure::new(f.pool.clone());
    let (approved, issued) = tokio::join!(
        approvals.approve_once(f.moderator, f.request, 1, "moderation"),
        grants.record(
            PositiveId::new(f.buyer).unwrap(),
            PositiveId::new(f.chapter).unwrap(),
            PositiveId::new(f.version).unwrap(),
            Some(PositiveId::new(f.purchase).unwrap()),
            DirectGrantKind::Chapter,
        ),
    );
    // Exactly one winner: refund wins and grant cannot be issued, or C-06
    // exposure wins and refund cannot issue any money.
    match (approved, issued) {
        (Ok(receipt), Err(DeliveryError::Denied)) => {
            assert_eq!(receipt.credited_minor, 300);
        }
        (Err(DirectRefundApproveError::AlreadyOpened), Ok(())) => {
            let balance: i64 = sqlx::query_scalar(
                "SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1",
            )
            .bind(f.buyer)
            .fetch_one(&f.pool)
            .await
            .unwrap();
            assert_eq!(balance, 700);
        }
        (a, b) => panic!("inconsistent open-vs-refund race: {a:?} vs {b:?}"),
    }
    d06_approval_cleanup(f).await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_refund_missing_creator_reversal_rolls_back_all_buyer_effects() {
    let f = d06_approval_fixture("d06_refund_failure_rollback", 86400).await;
    let reference = format!("d06:refund:{}", f.purchase);
    let mut tx = f.pool.begin().await.unwrap();
    // Intentionally simulate a broken writer: buyer credit, revocation,
    // platform reversal and terminal state, but OMIT creator reversal.
    sqlx::query(
        "UPDATE reader_wallet_accounts SET balance_minor=1000 WHERE user_id=$1",
    )
    .bind(f.buyer)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_wallet_entries
         (user_id,kind,delta_minor,balance_after_minor,reference,purchase_id)
         VALUES($1,'chapter_refund',300,1000,$2,$3)",
    )
    .bind(f.buyer)
    .bind(&reference)
    .bind(f.purchase)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_purchase_revocations
         (purchase_id,reversal_reference,reason)
         VALUES($1,$2,'refund')",
    )
    .bind(f.purchase)
    .bind(&reference)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO finance_direct_refund_postings
         (purchase_id,request_id,buyer_user_id,amount_minor,
          platform_reversal_minor,creator_reversal_minor,reversal_reference)
         VALUES($1,$2,$3,300,60,240,$4)",
    )
    .bind(f.purchase)
    .bind(f.request)
    .bind(f.buyer)
    .bind(&reference)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE finance_direct_refund_requests
         SET state='processing',revision=revision+1
         WHERE id=$1",
    )
    .bind(f.request)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE finance_direct_refund_requests
         SET state='approved',revision=revision+1,
             decision_actor_id=$2,decision_reason='approved',
             decision_reference=$3,decided_at=clock_timestamp()
         WHERE id=$1",
    )
    .bind(f.request)
    .bind(f.moderator)
    .bind(&reference)
    .execute(&mut *tx)
    .await
    .unwrap();
    assert!(tx.commit().await.is_err());

    let (balance, refund_entries, revocations, postings): (i64, i64, i64, i64) =
        sqlx::query_as(
            "SELECT a.balance_minor,
                    (SELECT count(*) FROM reader_wallet_entries e
                     WHERE e.purchase_id=$2 AND e.kind='chapter_refund'),
                    (SELECT count(*) FROM chapter_purchase_revocations v
                     WHERE v.purchase_id=$2),
                    (SELECT count(*) FROM finance_direct_refund_postings fp
                     WHERE fp.purchase_id=$2)
             FROM reader_wallet_accounts a WHERE a.user_id=$1",
        )
        .bind(f.buyer)
        .bind(f.purchase)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!((balance, refund_entries, revocations, postings), (700, 0, 0, 0));
    // The valid writer can retry after the failure without duplicate money.
    let correct = PgDirectRefundApprovals::new(f.pool.clone())
        .approve_once(f.moderator, f.request, 1, "approved")
        .await
        .unwrap();
    assert_eq!(correct.credited_minor, 300);
    assert!(!correct.replayed);
    d06_approval_cleanup(f).await;
}
