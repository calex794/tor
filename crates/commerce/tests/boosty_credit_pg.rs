//! D-10/S041: isolated PostgreSQL 18 evidence-to-wallet tests.
//! Never connect to the real TOR database or insert artificial real funding.
use sqlx::PgPool;
use tor_commerce::{
    domain::boosty_credit::{
        BoostyCreditError, BoostyCreditRepository, SubscriptionChargeKind,
        VerifiedSubscriptionCharge,
    },
    repositories::boosty_credit::PgBoostyCredits,
};

async fn fixture(suffix: &str) -> (PgPool, i64, i64, i64) {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL required");
    assert!(url.ends_with("/tor_ci"), "refuse non-tor_ci database");
    let pool = PgPool::connect(&url).await.unwrap();
    let payer: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Boosty CI payer','$argon2id$ci') RETURNING id",
    )
    .bind(format!("boosty_payer_{suffix}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let foreign: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Boosty CI foreign','$argon2id$ci') RETURNING id",
    )
    .bind(format!("boosty_foreign_{suffix}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    // The actual identity-proof flow belongs to a later adapter/auth slice.
    // This link is a controlled, explicitly labelled test-only fixture.
    let provider_id = payer + 8_000_000_000;
    sqlx::query(
        "INSERT INTO boosty_verified_wallet_links
         (provider_user_id,user_id,verified_by) VALUES($1,$2,'operator')",
    )
    .bind(provider_id)
    .bind(payer)
    .execute(&pool)
    .await
    .unwrap();
    (pool, payer, foreign, provider_id)
}

fn payment(
    id: &str,
    payer: i64,
    amount: i64,
    kind: SubscriptionChargeKind,
) -> VerifiedSubscriptionCharge {
    VerifiedSubscriptionCharge::new(id, payer, amount, kind).unwrap()
}

async fn balance(pool: &PgPool, user: i64) -> i64 {
    sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
        .bind(user)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn count(pool: &PgPool, table: &str, user: i64) -> i64 {
    let sql = match table {
        "credits" => "SELECT count(*) FROM boosty_subscription_credits WHERE user_id=$1",
        "ledger" => "SELECT count(*) FROM reader_wallet_entries WHERE user_id=$1",
        _ => panic!("test-only table selector"),
    };
    sqlx::query_scalar(sql)
        .bind(user)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_10_first_charge_and_renewal_credit_separately_but_replay_once() {
    let (pool, user, _, provider) = fixture("renewal").await;
    let repo = PgBoostyCredits::new(pool.clone());
    let first = payment(
        "ci-boosty-first-20261030",
        provider,
        8200,
        SubscriptionChargeKind::FirstPayment,
    );
    let renewal = payment(
        "ci-boosty-next-20261130",
        provider,
        8200,
        SubscriptionChargeKind::Renewal,
    );
    let one = repo.credit_once(&first).await.unwrap();
    assert!(!one.replayed);
    assert_eq!(one.balance_after_minor, 8200);
    let two = repo.credit_once(&renewal).await.unwrap();
    assert!(!two.replayed);
    assert_eq!(two.balance_after_minor, 16400);
    let replay = repo.credit_once(&first).await.unwrap();
    assert!(replay.replayed);
    assert_eq!(
        replay.balance_after_minor, 8200,
        "historic receipt is immutable"
    );
    assert_eq!(balance(&pool, user).await, 16400);
    assert_eq!(count(&pool, "credits", user).await, 2);
    assert_eq!(count(&pool, "ledger", user).await, 2);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_10_parallel_retries_and_mutated_payment_do_not_double_credit() {
    let (pool, user, _, provider) = fixture("race").await;
    let repo = PgBoostyCredits::new(pool.clone());
    let event = payment(
        "ci-boosty-race-20261030",
        provider,
        100,
        SubscriptionChargeKind::Renewal,
    );
    let (a, b) = tokio::join!(repo.credit_once(&event), repo.credit_once(&event));
    let a = a.unwrap();
    let b = b.unwrap();
    assert_ne!(a.replayed, b.replayed);
    assert_eq!(balance(&pool, user).await, 100);
    let tampered = payment(
        "ci-boosty-race-20261030",
        provider,
        999,
        SubscriptionChargeKind::Renewal,
    );
    assert_eq!(
        repo.credit_once(&tampered).await,
        Err(BoostyCreditError::ConflictingEvidence)
    );
    assert_eq!(balance(&pool, user).await, 100);
    assert_eq!(count(&pool, "credits", user).await, 1);
    assert_eq!(count(&pool, "ledger", user).await, 1);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_10_unlinked_payer_and_orphan_provider_claim_are_rejected() {
    let (pool, user, foreign, provider) = fixture("unlinked").await;
    let repo = PgBoostyCredits::new(pool.clone());
    let absent = payment(
        "ci-boosty-absent-20261030",
        provider + 99,
        400,
        SubscriptionChargeKind::FirstPayment,
    );
    assert_eq!(
        repo.credit_once(&absent).await,
        Err(BoostyCreditError::UnlinkedPayer)
    );
    assert_eq!(count(&pool, "credits", user).await, 0);
    let unrelated = payment(
        "ci-boosty-regular-20261030",
        provider,
        400,
        SubscriptionChargeKind::FirstPayment,
    );
    let receipt = repo.credit_once(&unrelated).await.unwrap();
    assert_eq!(receipt.user_id, user);
    assert_eq!(balance(&pool, user).await, 400);
    let foreign_balance: Option<i64> =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(foreign)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert_eq!(foreign_balance, None);
    // An unmatched source claim cannot be inserted without a wallet movement.
    let result = sqlx::query(
        "INSERT INTO boosty_subscription_credits
         (provider_operation_id,provider_user_id,user_id,charge_kind,
          amount_minor,charge_fingerprint,wallet_reference)
         VALUES($1,$2,$3,'subscribe',100,$4,$5)",
    )
    .bind("ci-boosty-orphan-20261030")
    .bind(provider)
    .bind(user)
    .bind(vec![0_u8; 32])
    .bind("boosty:orphan:20261030")
    .execute(&pool)
    .await;
    assert!(
        result.is_err(),
        "deferred trigger must reject missing ledger"
    );
}
