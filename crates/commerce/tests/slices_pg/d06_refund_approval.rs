use super::*;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use tor_commerce::repositories::{
    creator_finance::{CreatorFinanceError, PgCreatorFinance},
    direct_exposure::{DirectGrantKind, PgDirectFirstExposure},
    refund_approvals::{
        approval_step_up_payload_digest, DirectRefundApproveError, PgDirectRefundApprovals,
    },
    refund_requests::PgDirectRefundRequests,
};
use tor_contracts::paid_delivery::{DeliveryError, PositiveId};

pub(super) struct RefundFixture {
    pub(super) pool: PgPool,
    pub(super) buyer: i64,
    pub(super) moderator: i64,
    pub(super) chapter: i64,
    pub(super) version: i64,
    pub(super) purchase: i64,
    pub(super) request: i64,
}

pub(super) async fn d06_approval_fixture(tag: &str, window_seconds: i64) -> RefundFixture {
    let (pool, buyer, book, chapter, version) = fixture(tag, 1000, 300).await;
    let moderator: i64 = sqlx::query_scalar("SELECT owner_user_id FROM books WHERE id=$1")
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

pub(super) async fn issue_refund_step_up(f: &RefundFixture, reason: &str) -> ([u8; 32], String) {
    // D tests own the financial decision boundary only. A tests verify
    // password re-authentication and session-bound proof issuance.
    let session_digest: [u8; 32] =
        Sha256::digest(format!("d06:approval:{}", f.moderator).as_bytes()).into();
    let step_up_token = format!("{}8", "S".repeat(42));
    let proof_hash = Sha256::digest(step_up_token.as_bytes());
    let intent = approval_step_up_payload_digest(f.moderator, f.request, 1, reason);
    sqlx::query(
        "INSERT INTO sessions(
            user_id,token_hash,expires_at,step_up_proof_hash,step_up_scope,
            step_up_resource_id,step_up_revision,step_up_payload_digest,
            step_up_issued_at,step_up_expires_at)
         VALUES($1,$2,statement_timestamp()+INTERVAL '1 hour',
                $3,'finance.refund.approve.v1',$4,1,$5,
                statement_timestamp(),statement_timestamp()+INTERVAL '5 minutes')",
    )
    .bind(f.moderator)
    .bind(session_digest.as_slice())
    .bind(proof_hash.as_slice())
    .bind(f.request)
    .bind(intent.as_slice())
    .execute(&f.pool)
    .await
    .unwrap();
    (session_digest, step_up_token)
}

pub(super) async fn d06_approval_cleanup(f: RefundFixture) {
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
    let (session_digest, step_up_token) = issue_refund_step_up(&f, "approved").await;
    assert_eq!(
        refunds
            .approve_once(
                f.buyer,
                &session_digest,
                &step_up_token,
                f.request,
                1,
                "approved",
            )
            .await,
        Err(DirectRefundApproveError::Forbidden)
    );

    let first = refunds
        .approve_once(
            f.moderator,
            &session_digest,
            &step_up_token,
            f.request,
            1,
            "approved",
        )
        .await
        .unwrap();
    assert!(!first.replayed);
    assert_eq!(first.credited_minor, 300);
    assert_eq!(first.revision, 3);
    assert_eq!(first.purchase_id, f.purchase);
    let balance: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(f.buyer)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(balance, 1000);

    let (buyer_credit, platform_reverse, creator_reverse): (i64, i64, i64) = sqlx::query_as(
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
    assert_eq!(
        (buyer_credit, platform_reverse, creator_reverse),
        (300, 60, 240)
    );
    let reversed_creator: i64 = sqlx::query_scalar(
        "SELECT COALESCE(SUM(amount_minor),0)::BIGINT FROM finance_direct_refund_creator_reversals
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
        .approve_once(
            f.moderator,
            &session_digest,
            &step_up_token,
            f.request,
            1,
            "approved",
        )
        .await
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.credited_minor, 300);
    assert_eq!(replay.revision, 3);
    assert_eq!(
        refunds
            .approve_once(
                f.moderator,
                &session_digest,
                &step_up_token,
                f.request,
                1,
                "different reason",
            )
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
    let readable: bool = sqlx::query_scalar("SELECT tor_purchase_version_readable($1,$2,$3,$4)")
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

    let (session_digest, step_up_token) = issue_refund_step_up(&f, "not opened").await;
    let result = PgDirectRefundApprovals::new(f.pool.clone())
        .approve_once(
            f.moderator,
            &session_digest,
            &step_up_token,
            f.request,
            1,
            "not opened",
        )
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
    let balance: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
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
    let (session_digest, step_up_token) = issue_refund_step_up(&f, "too late").await;
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert_eq!(
        PgDirectRefundApprovals::new(f.pool.clone())
            .approve_once(
                f.moderator,
                &session_digest,
                &step_up_token,
                f.request,
                1,
                "too late",
            )
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
    let (session_digest, step_up_token) = issue_refund_step_up(&f, "moderation").await;
    let (approved, issued) = tokio::join!(
        approvals.approve_once(
            f.moderator,
            &session_digest,
            &step_up_token,
            f.request,
            1,
            "moderation",
        ),
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
    let (session_digest, step_up_token) = issue_refund_step_up(&f, "approved").await;
    let reference = format!("d06:refund:{}", f.purchase);
    let mut tx = f.pool.begin().await.unwrap();
    // Intentionally simulate a broken writer: buyer credit, revocation,
    // platform reversal and terminal state, but OMIT creator reversal.
    sqlx::query("UPDATE reader_wallet_accounts SET balance_minor=1000 WHERE user_id=$1")
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

    let (balance, refund_entries, revocations, postings): (i64, i64, i64, i64) = sqlx::query_as(
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
    assert_eq!(
        (balance, refund_entries, revocations, postings),
        (700, 0, 0, 0)
    );
    // The valid writer can retry after the failure without duplicate money.
    let correct = PgDirectRefundApprovals::new(f.pool.clone())
        .approve_once(
            f.moderator,
            &session_digest,
            &step_up_token,
            f.request,
            1,
            "approved",
        )
        .await
        .unwrap();
    assert_eq!(correct.credited_minor, 300);
    assert!(!correct.replayed);
    d06_approval_cleanup(f).await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_refund_step_up_is_bound_to_session_reason_and_expiry() {
    let f = d06_approval_fixture("d06_step_up_guards", 86400).await;
    let (session_digest, step_up_token) = issue_refund_step_up(&f, "confirmed").await;
    let approvals = PgDirectRefundApprovals::new(f.pool.clone());

    let foreign_session = [0_u8; 32];
    assert_eq!(
        approvals
            .approve_once(
                f.moderator,
                &foreign_session,
                &step_up_token,
                f.request,
                1,
                "confirmed",
            )
            .await,
        Err(DirectRefundApproveError::StepUpRequired)
    );
    assert_eq!(
        approvals
            .approve_once(
                f.moderator,
                &session_digest,
                &step_up_token,
                f.request,
                1,
                "different",
            )
            .await,
        Err(DirectRefundApproveError::InvalidStepUp)
    );
    sqlx::query(
        "UPDATE sessions
         SET step_up_issued_at=statement_timestamp()-INTERVAL '6 minutes',
             step_up_expires_at=statement_timestamp()-INTERVAL '1 minute'
         WHERE token_hash=$1",
    )
    .bind(session_digest.as_slice())
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        approvals
            .approve_once(
                f.moderator,
                &session_digest,
                &step_up_token,
                f.request,
                1,
                "confirmed",
            )
            .await,
        Err(DirectRefundApproveError::InvalidStepUp)
    );
    let (balance, reversals): (i64, i64) = sqlx::query_as(
        "SELECT a.balance_minor,
                (SELECT count(*) FROM finance_direct_refund_postings
                 WHERE purchase_id=$2)
         FROM reader_wallet_accounts a WHERE a.user_id=$1",
    )
    .bind(f.buyer)
    .bind(f.purchase)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!((balance, reversals), (700, 0));
    d06_approval_cleanup(f).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_refund_checkout_and_payout_reservation_share_three_connections() {
    use std::time::{Duration, Instant};

    let mut f = d06_approval_fixture("d06_refund_checkout_payout_contention", 86400).await;
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable tor_ci");
    assert!(url.ends_with("/tor_ci"), "never use a retained database");

    // The fixture is committed before the contention window. Replace its
    // default PgPool with the same bounded three-connection runtime shape.
    f.pool.close().await;
    f.pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&url)
        .await
        .expect("bounded isolated PostgreSQL");

    // The buyer is also a creator of a separate book, so payout reservation
    // uses real S038 authorization without changing the refund purchase's
    // already-pinned seller shares.
    sqlx::query(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
         VALUES($1,'ru','Refund contention owner','public',$2,'basic')",
    )
    .bind(format!("ci:d06:contention-owner:{}", f.buyer))
    .bind(f.buyer)
    .execute(&f.pool)
    .await
    .expect("seed independent creator relationship");

    let original_book: i64 = sqlx::query_scalar("SELECT book_id FROM chapters WHERE id=$1")
        .bind(f.chapter)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    let (new_chapter, new_version) = chapter_with_offer(&f.pool, original_book, 2, 300).await;
    let purchase_command = command(f.buyer, new_chapter, new_version, 300, 'L');
    let payout_key = IdempotencyKey::parse(&format!("{}8", "M".repeat(42))).unwrap();

    let (session_digest, step_up_token) = issue_refund_step_up(&f, "contention approval").await;
    let approval = PgDirectRefundApprovals::new(f.pool.clone());
    let finance = PgCreatorFinance::new(f.pool.clone());
    let checkout = PgCheckout::new(f.pool.clone()).with_creator_posting(true);

    // Initial wallet 1000 - historic purchase 300 = 700. The refund adds
    // 300. A 900 reservation and a new 300 purchase cannot both succeed:
    // their combined demand exceeds the final 1000 spendable balance.
    let gate = std::sync::Arc::new(tokio::sync::Barrier::new(4));
    let refund_gate = gate.clone();
    let reserve_gate = gate.clone();
    let purchase_gate = gate.clone();
    let probe_gate = gate.clone();

    let refund_action = async {
        refund_gate.wait().await;
        let start = Instant::now();
        let outcome = approval
            .approve_once(
                f.moderator,
                &session_digest,
                &step_up_token,
                f.request,
                1,
                "contention approval",
            )
            .await;
        (outcome, start.elapsed())
    };
    let reserve_action = async {
        reserve_gate.wait().await;
        let start = Instant::now();
        let outcome = finance.reserve_once(f.buyer, 900, &payout_key).await;
        (outcome, start.elapsed())
    };
    let purchase_action = async {
        purchase_gate.wait().await;
        let start = Instant::now();
        let outcome = checkout.checkout_once(&purchase_command).await;
        (outcome, start.elapsed())
    };
    let pool_probe = async {
        probe_gate.wait().await;
        let mut waits = Vec::new();
        for _ in 0..32 {
            let start = Instant::now();
            let connection = tokio::time::timeout(Duration::from_secs(3), f.pool.acquire())
                .await
                .expect("shared pool acquire must not hang")
                .expect("shared pool acquire must succeed");
            waits.push(start.elapsed());
            drop(connection);
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
        waits.sort_unstable();
        (waits[waits.len() * 95 / 100], waits[waits.len() - 1])
    };

    let (
        (refunded, refund_ms),
        (reserved, reserve_ms),
        (purchased, purchase_ms),
        (acquire_p95, acquire_max),
    ) = tokio::time::timeout(Duration::from_secs(25), async {
        tokio::join!(refund_action, reserve_action, purchase_action, pool_probe)
    })
    .await
    .expect("refund, purchase, or payout reservation deadlocked");

    let refund_receipt = refunded.expect("valid first refund must complete");
    assert_eq!(refund_receipt.credited_minor, 300);
    assert!(!refund_receipt.replayed);
    let reservation_won = reserved.is_ok();
    let purchase_won = purchased.is_ok();
    assert_eq!(
        u8::from(reservation_won) + u8::from(purchase_won),
        1,
        "money may not be reserved and spent twice: reservation={reserved:?}, purchase={purchased:?}"
    );
    if !reservation_won {
        assert_eq!(reserved, Err(CreatorFinanceError::Insufficient));
    }
    if !purchase_won {
        assert_eq!(purchased, Err(CheckoutError::InsufficientBalance));
    }
    assert!(
        acquire_p95 < Duration::from_secs(2),
        "shared pool acquisition p95 exceeded the contention budget"
    );

    let (wallet_balance, postings, refund_entries, purchase_count, ledger_delta): (
        i64,
        i64,
        i64,
        i64,
        i64,
    ) = sqlx::query_as(
        "SELECT w.balance_minor,
          (SELECT count(*) FROM finance_direct_refund_postings WHERE purchase_id=$2),
          (SELECT count(*) FROM reader_wallet_entries
           WHERE purchase_id=$2 AND kind='chapter_refund'),
          (SELECT count(*) FROM chapter_purchase_facts WHERE user_id=$1),
          (SELECT COALESCE(sum(delta_minor),0)::BIGINT
           FROM reader_wallet_entries WHERE user_id=$1)
         FROM reader_wallet_accounts w WHERE w.user_id=$1",
    )
    .bind(f.buyer)
    .bind(f.purchase)
    .fetch_one(&f.pool)
    .await
    .expect("read committed finance invariants");
    assert_eq!(postings, 1, "only one immutable refund posting");
    assert_eq!(refund_entries, 1, "only one refund wallet credit");
    assert_eq!(purchase_count, if purchase_won { 2 } else { 1 });
    assert_eq!(wallet_balance, 1000 + ledger_delta);
    assert_eq!(wallet_balance, if purchase_won { 700 } else { 1000 });

    let totals = finance.totals(f.buyer).await.unwrap();
    assert_eq!(totals.wallet_balance_minor, wallet_balance);
    assert_eq!(totals.reserved_minor, if reservation_won { 900 } else { 0 });
    assert_eq!(
        totals.available_minor,
        wallet_balance - totals.reserved_minor
    );
    assert!(totals.available_minor >= 0);

    println!(
        "TOR_MIXED_D06_S038_FINANCE_CONTENTION refund_ms={} reserve_ms={} purchase_ms={} pg_acquire_p95_ms={} pg_acquire_max_ms={} reservation_won={} purchase_won={}",
        refund_ms.as_millis(),
        reserve_ms.as_millis(),
        purchase_ms.as_millis(),
        acquire_p95.as_millis(),
        acquire_max.as_millis(),
        reservation_won,
        purchase_won
    );
    d06_approval_cleanup(f).await;
}
