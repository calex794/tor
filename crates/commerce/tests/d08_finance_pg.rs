//! D-08: real PostgreSQL atomic creator accounting tests. All tests are
//! ignored unless pointed at the disposable tor_ci database.
use sqlx::PgPool;
use tor_commerce::{
    domain::{
        batch_checkout::{BatchChapterOffer, BatchPurchaseCommand},
        money::{
            CheckoutError, CheckoutRepository, IdempotencyKey, MoneyMinor, PurchaseCommand,
            WalletId,
        },
    },
    repositories::{
        checkout::PgCheckout,
        creator_finance::{CreatorFinanceError, PgCreatorFinance},
        creator_wallet_credits::{CreatorWalletCreditError, PgCreatorWalletCredits},
    },
};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};

#[path = "d08_finance_pg/s038_wallet_race.rs"]
mod s038_wallet_race;

struct Fixture {
    pool: PgPool,
    buyer: i64,
    owner: i64,
    book: i64,
    offers: Vec<BatchChapterOffer>,
}

async fn chapter(pool: &PgPool, book: i64, number: i32, price: i64) -> BatchChapterOffer {
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,$2,'Финансовый тест','public','paid',$3) RETURNING id",
    )
    .bind(book)
    .bind(number)
    .bind(price)
    .fetch_one(pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'private d08 test') RETURNING id",
    )
    .bind(id)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(id)
        .bind(version)
        .execute(pool)
        .await
        .unwrap();
    BatchChapterOffer {
        edition: ChapterVersionRef {
            chapter_id: PositiveId::new(id).unwrap(),
            version_id: PositiveId::new(version).unwrap(),
        },
        expected_price: MoneyMinor::new(price).unwrap(),
    }
}

async fn fixture(name: &str) -> Fixture {
    let url = std::env::var("TEST_DATABASE_URL").expect("requires disposable tor_ci");
    assert!(
        url.ends_with("/tor_ci"),
        "D-08 refuses to operate on a non-disposable database"
    );
    let pool = PgPool::connect(&url).await.unwrap();
    let buyer: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'D08 buyer','$argon2id$ci') RETURNING id",
    )
    .bind(format!("d08_buyer_{name}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'D08 owner','$argon2id$ci') RETURNING id",
    )
    .bind(format!("d08_owner_{name}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES($1,1000,'RUB')",
    )
    .bind(buyer)
    .execute(&pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','D08 test book','public',$2) RETURNING id",
    )
    .bind(format!("ci:d08:{name}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let offers = vec![
        chapter(&pool, book, 1, 250).await,
        chapter(&pool, book, 2, 101).await,
        chapter(&pool, book, 3, 102).await,
    ];
    Fixture {
        pool,
        buyer,
        owner,
        book,
        offers,
    }
}

async fn publish_terms(f: &Fixture, version: &str, bps: i16) {
    sqlx::query(
        "INSERT INTO finance_revenue_terms
          (book_id,policy_version,platform_fee_bps,refund_policy_version,refund_window_seconds)
         VALUES($1,$2,$3,'ci-refund-7d-v1',604800)",
    )
    .bind(f.book)
    .bind(version)
    .bind(bps)
    .execute(&f.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO finance_revenue_term_states(book_id,policy_version)
         VALUES($1,$2)
         ON CONFLICT(book_id) DO UPDATE SET policy_version=EXCLUDED.policy_version",
    )
    .bind(f.book)
    .bind(version)
    .execute(&f.pool)
    .await
    .unwrap();
}

fn buy_one(f: &Fixture, offer: BatchChapterOffer, key: char) -> PurchaseCommand {
    PurchaseCommand::new(
        PositiveId::new(f.buyer).unwrap(),
        WalletId::new(f.buyer).unwrap(),
        offer.edition,
        offer.expected_price,
        IdempotencyKey::parse(&format!("{}8", key.to_string().repeat(42))).unwrap(),
    )
    .unwrap()
}

fn buy_batch(f: &Fixture, key: char) -> BatchPurchaseCommand {
    BatchPurchaseCommand::new(
        PositiveId::new(f.buyer).unwrap(),
        WalletId::new(f.buyer).unwrap(),
        f.offers[1..3].to_vec(),
        IdempotencyKey::parse(&format!("{}8", key.to_string().repeat(42))).unwrap(),
    )
    .unwrap()
}

async fn balance(f: &Fixture) -> i64 {
    sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
        .bind(f.buyer)
        .fetch_one(&f.pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s038_mature_creator_credit_is_journaled_once_in_unified_wallet() {
    let f = fixture("s038_wallet_credit").await;
    sqlx::query(
        "INSERT INTO finance_revenue_terms
         (book_id,policy_version,platform_fee_bps,refund_policy_version,refund_window_seconds)
         VALUES($1,'ci-s038-wallet-v1',2000,'ci-s038-wallet-refund-v1',1)",
    )
    .bind(f.book)
    .execute(&f.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO finance_revenue_term_states(book_id,policy_version)
         VALUES($1,'ci-s038-wallet-v1')",
    )
    .bind(f.book)
    .execute(&f.pool)
    .await
    .unwrap();
    PgCheckout::new(f.pool.clone())
        .with_creator_posting(true)
        .checkout_once(&buy_one(&f, f.offers[0], 'P'))
        .await
        .unwrap();

    let credits = PgCreatorWalletCredits::new(f.pool.clone());
    let finance = PgCreatorFinance::new(f.pool.clone());
    assert_eq!(
        credits.settle_mature_once(0, 20).await,
        Err(CreatorWalletCreditError::Invalid)
    );
    assert_eq!(
        credits.settle_mature_once(f.owner, 21).await,
        Err(CreatorWalletCreditError::Invalid)
    );
    assert_eq!(credits.settle_mature_once(f.owner, 20).await.unwrap(), 0);
    let before = finance.totals(f.owner).await.unwrap();
    assert_eq!(before.wallet_balance_minor, 0);
    assert_eq!(before.combined_balance_minor, 200);
    assert_eq!(before.available_minor, 0);

    sqlx::query("SELECT pg_sleep(1.2)")
        .execute(&f.pool)
        .await
        .unwrap();
    let matured = finance.totals(f.owner).await.unwrap();
    assert_eq!(matured.available_minor, 200);

    // Two app replicas race for one immutable recipient credit. The wallet
    // row and source PK serialize both physical and journal mutations.
    let (left, right) = tokio::join!(
        credits.settle_mature_once(f.owner, 20),
        credits.settle_mature_once(f.owner, 20)
    );
    assert_eq!(left.unwrap() + right.unwrap(), 1);
    assert_eq!(credits.settle_mature_once(f.owner, 20).await.unwrap(), 0);

    let owner_balance: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(f.owner)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(owner_balance, 200);
    let exact: (i64, i64, i64) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT,COALESCE(SUM(c.amount_minor),0)::BIGINT,
                COALESCE(SUM(j.delta_minor),0)::BIGINT
         FROM finance_creator_wallet_credits c
         JOIN reader_wallet_entries j ON j.id=c.wallet_entry_id
         WHERE c.user_id=$1 AND j.user_id=$1 AND j.kind='creator_earning_credit'",
    )
    .bind(f.owner)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(exact, (1, 200, 200));

    // Credit does not duplicate the old unposted earning in the finance view.
    let after = finance.totals(f.owner).await.unwrap();
    assert_eq!(
        (after.wallet_balance_minor, after.combined_balance_minor),
        (200, 200)
    );
    assert_eq!((after.mature_minor, after.available_minor), (0, 200));
    assert_eq!(
        finance.totals(f.buyer).await.unwrap().wallet_balance_minor,
        750
    );

    // No one can invent a stand-alone journal credit without its exact
    // immutable creator-earning source, even with a direct SQL insert.
    let mut forged = f.pool.begin().await.unwrap();
    sqlx::query("UPDATE reader_wallet_accounts SET balance_minor=balance_minor+1 WHERE user_id=$1")
        .bind(f.owner)
        .execute(&mut *forged)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO reader_wallet_entries
           (user_id,kind,delta_minor,balance_after_minor,reference)
         VALUES($1,'creator_earning_credit',1,201,$2)",
    )
    .bind(f.owner)
    .bind(format!("forged-s038-credit-{}", f.owner))
    .execute(&mut *forged)
    .await
    .unwrap();
    assert!(
        forged.commit().await.is_err(),
        "orphan creator credit must fail at commit"
    );
    assert_eq!(
        finance.totals(f.owner).await.unwrap().wallet_balance_minor,
        200
    );
    f.pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s038_reserved_unified_balance_blocks_both_single_and_batch_wallet_purchases() {
    let f = fixture("s038_wallet_hold").await;
    publish_terms(&f, "ci-s038-spend-hold-v1", 2000).await;
    sqlx::query(
        "INSERT INTO book_members(book_id,user_id,role,is_active)
         VALUES($1,$2,'translator',true)",
    )
    .bind(f.book)
    .bind(f.buyer)
    .execute(&f.pool)
    .await
    .unwrap();
    let finance = PgCreatorFinance::new(f.pool.clone());
    let payout_key = IdempotencyKey::parse(&format!("{}8", "Y".repeat(42))).unwrap();
    assert!(
        finance
            .reserve_once(f.buyer, 900, &payout_key)
            .await
            .unwrap()
            .created
    );
    assert_eq!(finance.totals(f.buyer).await.unwrap().available_minor, 100);

    let checkout = PgCheckout::new(f.pool.clone()).with_creator_posting(true);
    let blocked = checkout.checkout_once(&buy_one(&f, f.offers[0], 'Z')).await;
    assert!(
        matches!(blocked, Err(CheckoutError::InsufficientBalance)),
        "reserved wallet funds must decline as insufficient spendable balance"
    );
    assert_eq!(balance(&f).await, 1000);
    let posted: i64 =
        sqlx::query_scalar("SELECT COUNT(*)::BIGINT FROM chapter_purchase_facts WHERE user_id=$1")
            .bind(f.buyer)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(posted, 0, "a blocked wallet debit cannot leave a purchase");

    // A trusted rejection frees the hold; the very same purchase then works.
    let request: i64 = sqlx::query_scalar(
        "SELECT id FROM finance_creator_payout_requests WHERE user_id=$1 AND state='pending'",
    )
    .bind(f.buyer)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE finance_creator_payout_requests SET state='rejected',
             decided_at=clock_timestamp(),decision_actor_id=$2,
             decision_reason='CI hold release' WHERE id=$1",
    )
    .bind(request)
    .bind(f.owner)
    .execute(&f.pool)
    .await
    .unwrap();
    checkout
        .checkout_once(&buy_one(&f, f.offers[0], 'Z'))
        .await
        .unwrap();
    assert_eq!(balance(&f).await, 750);

    let second_key = IdempotencyKey::parse(&format!("{}8", "X".repeat(42))).unwrap();
    assert!(
        finance
            .reserve_once(f.buyer, 700, &second_key)
            .await
            .unwrap()
            .created
    );
    assert!(matches!(
        checkout.checkout_batch_once(&buy_batch(&f, 'V')).await,
        Err(
            tor_commerce::domain::batch_checkout::BatchCheckoutError::Checkout(
                CheckoutError::InsufficientBalance
            )
        )
    ));
    assert_eq!(balance(&f).await, 750);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*)::BIGINT FROM chapter_purchase_facts WHERE user_id=$1",
        )
        .bind(f.buyer)
        .fetch_one(&f.pool)
        .await
        .unwrap(),
        1
    );

    // One concurrent request and purchase must not each claim all 1000.
    // The same PostgreSQL protection also covers direct wallet debit SQL,
    // not only Rust's checkout functions.
    let direct = sqlx::query(
        "UPDATE reader_wallet_accounts SET balance_minor=balance_minor-100
         WHERE user_id=$1",
    )
    .bind(f.buyer)
    .execute(&f.pool)
    .await;
    assert!(
        direct.is_err(),
        "the database enforces remaining payout reserves"
    );

    f.pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_08_single_purchase_posts_exact_credit_and_hold_once() {
    let f = fixture("single").await;
    publish_terms(&f, "ci-finance-v1", 2000).await;
    let checkout = PgCheckout::new(f.pool.clone()).with_creator_posting(true);
    let command = buy_one(&f, f.offers[0], 'A');
    let purchased = checkout.checkout_once(&command).await.unwrap();
    assert!(!purchased.replayed);
    assert_eq!(balance(&f).await, 750);
    let (gross, fee, pool, exact_deadline, exact_hold): (i64, i64, i64, bool, bool) =
        sqlx::query_as(
            "SELECT s.gross_minor,s.platform_fee_minor,s.creator_pool_minor,
                    (s.refund_deadline_at=p.acquired_at + INTERVAL '604800 seconds'),
                    (e.clear_at=s.refund_deadline_at)
             FROM finance_chapter_postings s
             JOIN chapter_purchase_facts p ON p.id=s.purchase_id
             JOIN finance_creator_earnings e ON e.purchase_id=s.purchase_id
             WHERE s.purchase_id=$1 AND e.user_id=$2",
        )
        .bind(purchased.purchase_id.get())
        .bind(f.owner)
        .fetch_one(&f.pool)
        .await
        .unwrap();
    assert_eq!((gross, fee, pool), (250, 50, 200));
    assert!(exact_deadline && exact_hold);
    let snapshot: (bool, bool) = sqlx::query_as(
        "SELECT p.refund_policy_version=fp.refund_policy_version,
                p.refund_deadline_at=fp.refund_deadline_at
         FROM chapter_purchase_facts p
         JOIN finance_chapter_postings fp ON fp.purchase_id=p.id
         WHERE p.id=$1",
    )
    .bind(purchased.purchase_id.get())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(snapshot, (true, true));
    let creator_amount: i64 = sqlx::query_scalar(
        "SELECT amount_minor FROM finance_creator_earnings
         WHERE purchase_id=$1 AND user_id=$2",
    )
    .bind(purchased.purchase_id.get())
    .bind(f.owner)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(creator_amount, 200);
    let platform: i64 = sqlx::query_scalar(
        "SELECT amount_minor FROM finance_platform_credits WHERE purchase_id=$1",
    )
    .bind(purchased.purchase_id.get())
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(platform, 50);
    let replay = checkout.checkout_once(&command).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(replay.purchase_id, purchased.purchase_id);
    assert_eq!(balance(&f).await, 750);
    let rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM finance_creator_earnings WHERE purchase_id=$1")
            .bind(purchased.purchase_id.get())
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(rows, 1);
    f.pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn s038_creator_cash_origin_refund_hold_retry_and_concurrent_reservation() {
    let f = fixture("s038").await;
    // One-second explicitly approved CI test term permits an actual maturity
    // boundary without modifying immutable purchase or earning timestamps.
    sqlx::query(
        "INSERT INTO finance_revenue_terms
         (book_id,policy_version,platform_fee_bps,refund_policy_version,refund_window_seconds)
         VALUES($1,'ci-s038-v1',2000,'ci-s038-refund-v1',1)",
    )
    .bind(f.book)
    .execute(&f.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO finance_revenue_term_states(book_id,policy_version)
         VALUES($1,'ci-s038-v1')",
    )
    .bind(f.book)
    .execute(&f.pool)
    .await
    .unwrap();
    let receipt = PgCheckout::new(f.pool.clone())
        .with_creator_posting(true)
        .checkout_once(&buy_one(&f, f.offers[0], 'N'))
        .await
        .unwrap();
    let repo = PgCreatorFinance::new(f.pool.clone());
    let earnings = repo.recent_earnings(f.owner).await.unwrap();
    assert_eq!(earnings.len(), 1);
    assert_eq!(earnings[0].purchase_id, receipt.purchase_id.get());
    assert_eq!(earnings[0].amount_minor, 200);
    assert_eq!(earnings[0].status, "immature");
    assert!(repo.recent_earnings(f.buyer).await.unwrap().is_empty());
    assert!(repo.recent_refunds(f.owner).await.unwrap().is_empty());
    assert!(repo.recent_refunds(f.buyer).await.unwrap().is_empty());
    let before = repo.totals(f.owner).await.unwrap();
    assert_eq!(before.gross_minor, 200);
    assert_eq!(before.immature_minor, 200);
    assert_eq!(before.available_minor, 0);
    let buyer_totals = repo.totals(f.buyer).await.unwrap();
    assert_eq!(buyer_totals.gross_minor, 0);
    assert_eq!(buyer_totals.wallet_balance_minor, 750);
    assert_eq!(buyer_totals.combined_balance_minor, 750);
    assert_eq!(buyer_totals.available_minor, 750);
    // NEwBOO: wallet funds are not filtered by origin. Request permission
    // still requires a current creator/team relationship.
    let buyer_key = IdempotencyKey::parse(&format!("{}8", "V".repeat(42))).unwrap();
    assert_eq!(
        repo.reserve_once(f.buyer, 100, &buyer_key).await,
        Err(CreatorFinanceError::Forbidden)
    );
    assert_eq!(balance(&f).await, 750);

    sqlx::query("SELECT pg_sleep(1.2)")
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(repo.totals(f.owner).await.unwrap().available_minor, 200);
    assert_eq!(
        repo.recent_earnings(f.owner).await.unwrap()[0].status,
        "mature"
    );

    // A pending buyer refund holds back the *whole* original creator earning.
    let refund: i64 = sqlx::query_scalar(
        "INSERT INTO finance_direct_refund_requests
         (purchase_id,user_id,request_key,request_digest,reason)
         VALUES($1,$2,$3,decode(repeat('08',32),'hex'),'CI valid pending refund')
         RETURNING id",
    )
    .bind(receipt.purchase_id.get())
    .bind(f.buyer)
    .bind(format!("{}8", "R".repeat(42)))
    .fetch_one(&f.pool)
    .await
    .unwrap();
    let held = repo.totals(f.owner).await.unwrap();
    assert_eq!(
        (
            held.mature_minor,
            held.open_refund_hold_minor,
            held.available_minor
        ),
        (200, 200, 0)
    );
    assert_eq!(
        repo.recent_earnings(f.owner).await.unwrap()[0].status,
        "refund_hold"
    );
    let key = IdempotencyKey::parse(&format!("{}8", "S".repeat(42))).unwrap();
    assert_eq!(
        repo.reserve_once(f.owner, 120, &key).await,
        Err(CreatorFinanceError::Insufficient)
    );
    // Simulate a trusted cancellation of the refund fixture (not a public
    // moderator endpoint) to release the currently held creator cash.
    sqlx::query(
        "UPDATE finance_direct_refund_requests SET state='rejected',
             revision=revision+1,decided_at=clock_timestamp(),
             decision_actor_id=$2,decision_reason='CI rejected',
             decision_reference=$3 WHERE id=$1",
    )
    .bind(refund)
    .bind(f.owner)
    .bind(format!("ci-s038-reject-{}", receipt.purchase_id.get()))
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(repo.totals(f.owner).await.unwrap().available_minor, 200);

    let first = repo.reserve_once(f.owner, 120, &key).await.unwrap();
    assert!(first.created);
    let replay = repo.reserve_once(f.owner, 120, &key).await.unwrap();
    assert_eq!(first.id, replay.id);
    assert!(!replay.created);
    assert_eq!(
        repo.reserve_once(f.owner, 121, &key).await,
        Err(CreatorFinanceError::Conflict)
    );
    // Two competing 60-kopeck requests cannot both reserve the remaining 80.
    let key_a = IdempotencyKey::parse(&format!("{}8", "T".repeat(42))).unwrap();
    let key_b = IdempotencyKey::parse(&format!("{}8", "U".repeat(42))).unwrap();
    let (a, b) = tokio::join!(
        repo.reserve_once(f.owner, 60, &key_a),
        repo.reserve_once(f.owner, 60, &key_b)
    );
    assert_eq!(a.is_ok() as i32 + b.is_ok() as i32, 1);
    assert!(
        a == Err(CreatorFinanceError::Insufficient) || b == Err(CreatorFinanceError::Insufficient)
    );
    let after = repo.totals(f.owner).await.unwrap();
    assert_eq!((after.reserved_minor, after.available_minor), (180, 20));
    assert_eq!(repo.history(f.owner).await.unwrap().len(), 2);
    assert!(repo.history(f.buyer).await.unwrap().is_empty());
    assert_eq!(
        balance(&f).await,
        750,
        "pending payouts never debit wallets"
    );

    let bad = sqlx::query(
        "UPDATE finance_creator_payout_requests
         SET amount_minor=amount_minor+1 WHERE id=$1",
    )
    .bind(first.id)
    .execute(&f.pool)
    .await;
    assert!(bad.is_err(), "immutable request cannot be silently changed");
    // A later trusted rejection releases its *own* reservation without
    // revising the historical creator earning or originating purchase.
    sqlx::query(
        "UPDATE finance_creator_payout_requests
         SET state='rejected',decided_at=clock_timestamp(),
             decision_actor_id=$2,decision_reason='CI release'
         WHERE id=$1",
    )
    .bind(first.id)
    .bind(f.buyer)
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(repo.totals(f.owner).await.unwrap().available_minor, 140);
    let exact = repo.reserve_once(f.owner, 120, &key).await.unwrap();
    assert_eq!(exact.id, first.id, "rejected key replay is still history");
    assert!(!exact.created);
    assert_eq!(repo.totals(f.owner).await.unwrap().available_minor, 140);

    // Verified funding and creator earnings share the same NEwBOO-style
    // payout capacity. A top-up is not excluded by source classification.
    sqlx::query(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES($1,1000,'RUB')",
    )
    .bind(f.owner)
    .execute(&f.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_wallet_entries
           (user_id,kind,delta_minor,balance_after_minor,reference)
         VALUES($1,'verified_funding',1000,1000,$2)",
    )
    .bind(f.owner)
    .bind(format!("s038-unified-wallet-owner-{}", f.owner))
    .execute(&f.pool)
    .await
    .unwrap();
    let unified = repo.totals(f.owner).await.unwrap();
    assert_eq!(unified.wallet_balance_minor, 1000);
    assert_eq!(unified.combined_balance_minor, 1200);
    assert_eq!(unified.immature_minor, 0);
    assert_eq!(unified.reserved_minor, 60);
    assert_eq!(unified.available_minor, 1140);
    let wallet_key = IdempotencyKey::parse(&format!("{}8", "W".repeat(42))).unwrap();
    assert!(
        repo.reserve_once(f.owner, 800, &wallet_key)
            .await
            .unwrap()
            .created
    );
    assert_eq!(repo.totals(f.owner).await.unwrap().available_minor, 340);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM finance_creator_payout_requests
             WHERE user_id=$1 AND source_origin='unified_wallet'"
        )
        .bind(f.owner)
        .fetch_one(&f.pool)
        .await
        .unwrap(),
        3
    );
    f.pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_08_missing_published_terms_fails_closed_before_debit() {
    let f = fixture("missing").await;
    let checkout = PgCheckout::new(f.pool.clone()).with_creator_posting(true);
    assert_eq!(
        checkout.checkout_once(&buy_one(&f, f.offers[0], 'B')).await,
        Err(CheckoutError::Unavailable)
    );
    assert_eq!(balance(&f).await, 1000);
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM chapter_purchase_facts WHERE user_id=$1")
            .bind(f.buyer)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    f.pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_08_batch_postings_are_atomic_and_replayed_without_duplication() {
    let f = fixture("batch").await;
    publish_terms(&f, "ci-finance-batch-v1", 2000).await;
    let checkout = PgCheckout::new(f.pool.clone()).with_creator_posting(true);
    let command = buy_batch(&f, 'C');
    let result = checkout.checkout_batch_once(&command).await.unwrap();
    assert_eq!(result.total.get(), 203);
    assert_eq!(balance(&f).await, 797);
    assert_eq!(result.purchases.len(), 2);
    let rows: Vec<(i64, i64, i64)> = sqlx::query_as(
        "SELECT p.amount_minor,s.platform_fee_minor,e.amount_minor
         FROM finance_chapter_postings s
         JOIN chapter_purchase_facts p ON p.id=s.purchase_id
         JOIN finance_creator_earnings e ON e.purchase_id=s.purchase_id
         WHERE p.user_id=$1 ORDER BY p.amount_minor",
    )
    .bind(f.buyer)
    .fetch_all(&f.pool)
    .await
    .unwrap();
    assert_eq!(rows, vec![(101, 20, 81), (102, 20, 82)]);
    let replay = checkout.checkout_batch_once(&command).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(
        replay.purchases[0].purchase_id,
        result.purchases[0].purchase_id
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM finance_creator_earnings e
         JOIN chapter_purchase_facts p ON p.id=e.purchase_id
         WHERE p.user_id=$1",
    )
    .bind(f.buyer)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(count, 2);
    assert_eq!(balance(&f).await, 797);
    f.pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_08_missing_hold_rejects_commit_and_rolls_back_wallet_and_rights() {
    let f = fixture("rollback").await;
    publish_terms(&f, "ci-finance-rollback-v1", 2000).await;
    let mut tx = f.pool.begin().await.unwrap();
    sqlx::query(
        "UPDATE reader_wallet_accounts SET balance_minor=balance_minor-250 WHERE user_id=$1",
    )
    .bind(f.buyer)
    .execute(&mut *tx)
    .await
    .unwrap();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_purchase_facts
          (user_id,chapter_id,version_id,settlement_reference,amount_minor,currency)
         VALUES($1,$2,$3,$4,250,'RUB') RETURNING id",
    )
    .bind(f.buyer)
    .bind(f.offers[0].edition.chapter_id.get())
    .bind(f.offers[0].edition.version_id.get())
    .bind(format!("d08-rollback-wallet-{}", f.buyer))
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_purchase_share_snapshots
          (purchase_id,book_id,share_revision,gross_amount_minor) VALUES($1,$2,1,250)",
    )
    .bind(id)
    .bind(f.book)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chapter_purchase_share_lines(purchase_id,user_id,share_percent)
         VALUES($1,$2,100)",
    )
    .bind(id)
    .bind(f.owner)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_wallet_entries
          (user_id,kind,delta_minor,balance_after_minor,reference,purchase_id)
         VALUES($1,'chapter_purchase',-250,750,$2,$3)",
    )
    .bind(f.buyer)
    .bind(format!("d08-rollback-wallet-{}", f.buyer))
    .bind(id)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO finance_chapter_postings
          (purchase_id,book_id,share_revision,policy_version,refund_policy_version,
           refund_deadline_at,gross_minor,platform_fee_minor,creator_pool_minor)
         SELECT id,$2,1,'ci-finance-rollback-v1','ci-refund-7d-v1',
                acquired_at + INTERVAL '604800 seconds',250,50,200
         FROM chapter_purchase_facts WHERE id=$1",
    )
    .bind(id)
    .bind(f.book)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query("INSERT INTO finance_platform_credits(purchase_id,amount_minor) VALUES($1,50)")
        .bind(id)
        .execute(&mut *tx)
        .await
        .unwrap();

    // Intentionally OMIT the creator's authoritative credit/hold. The
    // deferred finance invariant must reject the whole transaction.
    assert!(tx.commit().await.is_err());
    assert_eq!(balance(&f).await, 1000);
    for query in [
        "SELECT COUNT(*) FROM chapter_purchase_facts WHERE user_id=$1",
        "SELECT COUNT(*) FROM library_purchased_chapters WHERE user_id=$1",
        "SELECT COUNT(*) FROM reader_wallet_entries WHERE user_id=$1",
    ] {
        let count: i64 = sqlx::query_scalar(query)
            .bind(f.buyer)
            .fetch_one(&f.pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }
    f.pool.close().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_08_immutable_share_revision_keeps_historic_creator_amounts() {
    let f = fixture("split").await;
    publish_terms(&f, "ci-finance-split-v1", 2000).await;
    let translator: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES('d08_translator_split@example.test','D08 translator','$argon2id$ci')
         RETURNING id",
    )
    .fetch_one(&f.pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_members(book_id,user_id,role,is_active)
         VALUES($1,$2,'translator',true)",
    )
    .bind(f.book)
    .bind(translator)
    .execute(&f.pool)
    .await
    .unwrap();

    let mut tx = f.pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO revenue_share_versions(book_id,revision,changed_by_user_id,reason)
         VALUES($1,2,$2,'d08_split_test')",
    )
    .bind(f.book)
    .bind(f.owner)
    .execute(&mut *tx)
    .await
    .unwrap();
    for user in [f.owner, translator] {
        sqlx::query(
            "INSERT INTO revenue_share_assignments(book_id,revision,user_id,share_percent)
             VALUES($1,2,$2,50)",
        )
        .bind(f.book)
        .bind(user)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    sqlx::query(
        "UPDATE revenue_share_states SET current_revision=2
         WHERE book_id=$1 AND current_revision=1",
    )
    .bind(f.book)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let checkout = PgCheckout::new(f.pool.clone()).with_creator_posting(true);
    let first = checkout
        .checkout_once(&buy_one(&f, f.offers[1], 'D'))
        .await
        .unwrap();
    let amounts: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT user_id,amount_minor FROM finance_creator_earnings
         WHERE purchase_id=$1 ORDER BY user_id",
    )
    .bind(first.purchase_id.get())
    .fetch_all(&f.pool)
    .await
    .unwrap();
    assert_eq!(amounts, vec![(f.owner, 41), (translator, 40)]);

    let mut tx = f.pool.begin().await.unwrap();
    sqlx::query(
        "INSERT INTO revenue_share_versions(book_id,revision,changed_by_user_id,reason)
         VALUES($1,3,$2,'d08_new_split_test')",
    )
    .bind(f.book)
    .bind(f.owner)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO revenue_share_assignments(book_id,revision,user_id,share_percent)
         VALUES($1,3,$2,100)",
    )
    .bind(f.book)
    .bind(f.owner)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE revenue_share_states SET current_revision=3
         WHERE book_id=$1 AND current_revision=2",
    )
    .bind(f.book)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let later = checkout
        .checkout_once(&buy_one(&f, f.offers[2], 'E'))
        .await
        .unwrap();
    let next_amount: i64 = sqlx::query_scalar(
        "SELECT amount_minor FROM finance_creator_earnings
         WHERE purchase_id=$1 AND user_id=$2",
    )
    .bind(later.purchase_id.get())
    .bind(f.owner)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(next_amount, 82);
    let old: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT user_id,amount_minor FROM finance_creator_earnings
         WHERE purchase_id=$1 ORDER BY user_id",
    )
    .bind(first.purchase_id.get())
    .fetch_all(&f.pool)
    .await
    .unwrap();
    assert_eq!(old, amounts, "later Book shares may not rewrite old money");
    f.pool.close().await;
}
