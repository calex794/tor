//! Repeated multi-wallet financial contention with the real transactional ports.
//! This is a disposable /tor_ci-only regression, not a production load SLO.
use super::d06_refund_approval::{d06_approval_fixture, issue_refund_step_up};
use super::*;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{sync::Barrier, task::JoinSet};
use tor_commerce::repositories::{
    creator_finance::{CreatorFinanceError, PgCreatorFinance},
    refund_approvals::PgDirectRefundApprovals,
};

const WALLETS: usize = 4;
const ROUNDS: usize = 3;
const PAYOUT_MINOR: i64 = 900;
const PRICE_MINOR: i64 = 300;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_06_multi_wallet_three_round_finance_contention() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable database required");
    assert!(url.ends_with("/tor_ci"), "never touch retained PostgreSQL");
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&url)
        .await
        .expect("bounded disposable PostgreSQL pool");

    let mut durations = Vec::with_capacity(WALLETS * ROUNDS * 3);
    let mut acquires = Vec::with_capacity(ROUNDS * 24);
    let mut reservation_winners = 0_usize;
    let mut purchase_winners = 0_usize;

    for round in 0..ROUNDS {
        let gate = Arc::new(Barrier::new(WALLETS + 2));
        let mut tasks = JoinSet::new();

        for wallet in 0..WALLETS {
            let tag = format!("d06_multi_wallet_{round}_{wallet}");
            let mut f = d06_approval_fixture(&tag, 86400).await;
            // Close the fixture pool and reuse precisely one three-slot pool
            // for all real checkout, refund, reservation and SQL probes.
            f.pool.close().await;
            f.pool = pool.clone();
            sqlx::query(
                "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id,access_level)
                 VALUES($1,'ru','Independent payout creator','public',$2,'basic')",
            )
            .bind(format!("ci:d06:multi-owner:{}:{round}", f.buyer))
            .bind(f.buyer)
            .execute(&pool)
            .await
            .expect("seed independent creator relation");

            let original_book: i64 = sqlx::query_scalar("SELECT book_id FROM chapters WHERE id=$1")
                .bind(f.chapter)
                .fetch_one(&pool)
                .await
                .unwrap();
            let (second_chapter, second_version) =
                chapter_with_offer(&pool, original_book, 2, PRICE_MINOR).await;
            let purchase = command(f.buyer, second_chapter, second_version, PRICE_MINOR, 'L');
            let payout_key = IdempotencyKey::parse(&format!("{}8", "M".repeat(42))).unwrap();
            let reason = format!("round {round} wallet {wallet} approval");
            let (session_digest, proof) = issue_refund_step_up(&f, &reason).await;

            let gate = Arc::clone(&gate);
            let db = pool.clone();
            tasks.spawn(async move {
                let approval = PgDirectRefundApprovals::new(db.clone());
                let finance = PgCreatorFinance::new(db.clone());
                let checkout = PgCheckout::new(db.clone()).with_creator_posting(true);
                gate.wait().await;

                let refund_action = async {
                    let start = Instant::now();
                    let outcome = approval
                        .approve_once(
                            f.moderator,
                            &session_digest,
                            &proof,
                            f.request,
                            1,
                            &reason,
                        )
                        .await;
                    (outcome, start.elapsed())
                };
                let reserve_action = async {
                    let start = Instant::now();
                    let outcome = finance.reserve_once(f.buyer, PAYOUT_MINOR, &payout_key).await;
                    (outcome, start.elapsed())
                };
                let purchase_action = async {
                    let start = Instant::now();
                    let outcome = checkout.checkout_once(&purchase).await;
                    (outcome, start.elapsed())
                };
                let ((refunded, refund_elapsed), (reserved, reserve_elapsed), (purchased, purchase_elapsed)) =
                    tokio::join!(refund_action, reserve_action, purchase_action);

                let receipt = refunded.expect("the valid refund must complete");
                assert_eq!(receipt.credited_minor, PRICE_MINOR);
                assert!(!receipt.replayed);
                let reserve_won = reserved.is_ok();
                let purchase_won = purchased.is_ok();
                assert_eq!(
                    u8::from(reserve_won) + u8::from(purchase_won),
                    1,
                    "wallet {wallet}, round {round}: double-spend or lost checkout: {reserved:?} / {purchased:?}"
                );
                if !reserve_won {
                    assert_eq!(reserved, Err(CreatorFinanceError::Insufficient));
                }
                if !purchase_won {
                    assert_eq!(purchased, Err(CheckoutError::InsufficientBalance));
                }

                let (balance, refunds, credits, purchases, ledger_delta, reservations): (i64, i64, i64, i64, i64, i64) =
                    sqlx::query_as(
                        "SELECT w.balance_minor,
                            (SELECT COUNT(*) FROM finance_direct_refund_postings WHERE purchase_id=$2),
                            (SELECT COUNT(*) FROM reader_wallet_entries
                             WHERE purchase_id=$2 AND kind='chapter_refund'),
                            (SELECT COUNT(*) FROM chapter_purchase_facts WHERE user_id=$1),
                            (SELECT COALESCE(SUM(delta_minor),0)::BIGINT
                             FROM reader_wallet_entries WHERE user_id=$1),
                            (SELECT COUNT(*) FROM finance_creator_payout_requests
                             WHERE user_id=$1 AND state='pending')
                         FROM reader_wallet_accounts w WHERE w.user_id=$1",
                    )
                    .bind(f.buyer)
                    .bind(f.purchase)
                    .fetch_one(&db)
                    .await
                    .expect("verify committed financial ledger");
                assert_eq!((refunds, credits), (1, 1));
                assert_eq!(purchases, if purchase_won { 2 } else { 1 });
                assert_eq!(reservations, if reserve_won { 1 } else { 0 });
                assert_eq!(balance, 1000 + ledger_delta);
                assert_eq!(balance, if purchase_won { 700 } else { 1000 });

                let totals = finance.totals(f.buyer).await.unwrap();
                assert_eq!(totals.wallet_balance_minor, balance);
                assert_eq!(totals.reserved_minor, if reserve_won { PAYOUT_MINOR } else { 0 });
                assert_eq!(totals.available_minor, balance - totals.reserved_minor);
                assert!(totals.available_minor >= 0);

                // Replays must not issue a second refund, reserve twice, or debit the buyer twice.
                let repeat = approval
                    .approve_once(
                        f.moderator,
                        &session_digest,
                        &proof,
                        f.request,
                        1,
                        &reason,
                    )
                    .await
                    .expect("replay of approved refund");
                assert!(repeat.replayed);
                if let Ok(first) = reserved {
                    let repeat = finance
                        .reserve_once(f.buyer, PAYOUT_MINOR, &payout_key)
                        .await
                        .expect("replay payout reservation");
                    assert_eq!(repeat.id, first.id);
                    assert!(!repeat.created);
                }
                if let Ok(first) = purchased {
                    let repeat = checkout
                        .checkout_once(&purchase)
                        .await
                        .expect("replay winning checkout");
                    assert_eq!(repeat.purchase_id, first.purchase_id);
                    assert!(repeat.replayed);
                }
                let (balance_after_replays, purchase_facts_after_replays, refund_postings_after_replays):
                    (i64, i64, i64) = sqlx::query_as(
                        "SELECT w.balance_minor,
                           (SELECT COUNT(*) FROM chapter_purchase_facts WHERE user_id=$1),
                           (SELECT COUNT(*) FROM finance_direct_refund_postings WHERE purchase_id=$2)
                         FROM reader_wallet_accounts w WHERE w.user_id=$1",
                    )
                    .bind(f.buyer)
                    .bind(f.purchase)
                    .fetch_one(&db)
                    .await
                    .expect("verify financial replay idempotence");
                assert_eq!(balance_after_replays, balance);
                assert_eq!(purchase_facts_after_replays, purchases);
                assert_eq!(refund_postings_after_replays, 1);

                sqlx::query(
                    "UPDATE account_roles SET is_active=FALSE
                     WHERE user_id=$1 AND role='administrator'",
                )
                .bind(f.moderator)
                .execute(&db)
                .await
                .expect("deactivate disposable administrator");
                ([refund_elapsed, reserve_elapsed, purchase_elapsed], reserve_won, purchase_won)
            });
        }

        let gate_probe = Arc::clone(&gate);
        let probe_pool = pool.clone();
        let probe = tokio::spawn(async move {
            gate_probe.wait().await;
            let mut timings = Vec::new();
            for _ in 0..24 {
                let started = Instant::now();
                let connection = tokio::time::timeout(Duration::from_secs(3), probe_pool.acquire())
                    .await
                    .expect("pool acquire did not finish")
                    .expect("pool acquired an available connection");
                timings.push(started.elapsed());
                drop(connection);
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            timings
        });
        gate.wait().await;

        let round_results = tokio::time::timeout(Duration::from_secs(45), async {
            let mut values = Vec::new();
            while let Some(joined) = tasks.join_next().await {
                values.push(joined.expect("finance race lane panicked"));
            }
            values
        })
        .await
        .expect("multi-wallet transactions deadlocked");
        assert_eq!(round_results.len(), WALLETS);
        for (latency, reserve_won, purchase_won) in round_results {
            durations.extend(latency);
            reservation_winners += usize::from(reserve_won);
            purchase_winners += usize::from(purchase_won);
        }
        acquires.extend(probe.await.expect("pool probe joined"));
    }

    assert_eq!(reservation_winners + purchase_winners, WALLETS * ROUNDS);
    durations.sort_unstable();
    acquires.sort_unstable();
    let operation_p95 = durations[durations.len() * 95 / 100];
    let operation_max = durations[durations.len() - 1];
    let acquire_p95 = acquires[acquires.len() * 95 / 100];
    let acquire_max = acquires[acquires.len() - 1];
    println!(
        "TOR_MULTI_WALLET_D06_S038_CONTENTION wallets={} rounds={} operations={} reserve_wins={} purchase_wins={} financial_p95_ms={} financial_max_ms={} pg_acquire_p95_ms={} pg_acquire_max_ms={}",
        WALLETS,
        ROUNDS,
        durations.len(),
        reservation_winners,
        purchase_winners,
        operation_p95.as_millis(),
        operation_max.as_millis(),
        acquire_p95.as_millis(),
        acquire_max.as_millis(),
    );
    assert!(
        operation_p95 < Duration::from_secs(3),
        "financial p95 budget exceeded"
    );
    assert!(
        operation_max < Duration::from_secs(5),
        "an individual financial operation exceeded the tail latency budget"
    );
    assert!(
        acquire_p95 < Duration::from_secs(2),
        "PgPool p95 budget exceeded"
    );
    pool.close().await;
}
