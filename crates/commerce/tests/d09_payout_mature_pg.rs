//! S039: unposted mature creator earnings are credited and debited in one
//! disposable PostgreSQL payout transaction, never as an external transfer.
#![forbid(unsafe_code)]
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tor_commerce::{
    domain::money::{CheckoutRepository, IdempotencyKey, MoneyMinor, PurchaseCommand, WalletId},
    repositories::{
        checkout::PgCheckout,
        creator_finance::PgCreatorFinance,
        creator_payout_posting::{payout_step_up_payload_digest, PgCreatorPayoutPosting},
    },
};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_09_post_materializes_mature_earnings_in_same_commit() {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable tor_ci");
    assert!(url.ends_with("/tor_ci"), "never touch retained PostgreSQL");
    let pool = PgPool::connect(&url).await.unwrap();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    let mut ids = Vec::new();
    for (label, name) in [
        ("buyer", "S039 payer"),
        ("owner", "S039 author"),
        ("admin", "S039 administrator"),
    ] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,$2,'$argon2id$ci') RETURNING id",
        )
        .bind(format!("s039-mature-{label}-{stamp}@example.invalid"))
        .bind(name)
        .fetch_one(&pool)
        .await
        .unwrap();
        ids.push(id);
    }
    let (buyer, owner, admin) = (ids[0], ids[1], ids[2]);
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
         VALUES($1,'ru','Mature credits','public',$2) RETURNING id",
    )
    .bind(format!("ci:s039:mature:{stamp}"))
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    let chapter: i64 = sqlx::query_scalar(
        "INSERT INTO chapters(book_id,chapter_no,title,visibility,access_level,price_amount)
         VALUES($1,1,'Mature credit chapter','public','paid',250) RETURNING id",
    )
    .bind(book)
    .fetch_one(&pool)
    .await
    .unwrap();
    let version: i64 = sqlx::query_scalar(
        "INSERT INTO chapter_versions(chapter_id,version_no,body_markdown)
         VALUES($1,1,'mature-credit-fixture') RETURNING id",
    )
    .bind(chapter)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO chapter_publications(chapter_id,version_id) VALUES($1,$2)")
        .bind(chapter)
        .bind(version)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO finance_revenue_terms
         (book_id,policy_version,platform_fee_bps,refund_policy_version,refund_window_seconds)
         VALUES($1,'ci-s039-mature-v1',2000,'ci-s039-mature-refund-v1',1)",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO finance_revenue_term_states(book_id,policy_version)
         VALUES($1,'ci-s039-mature-v1')",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();

    let purchase = PurchaseCommand::new(
        PositiveId::new(buyer).unwrap(),
        WalletId::new(buyer).unwrap(),
        ChapterVersionRef {
            chapter_id: PositiveId::new(chapter).unwrap(),
            version_id: PositiveId::new(version).unwrap(),
        },
        MoneyMinor::new(250).unwrap(),
        IdempotencyKey::parse(&format!("{}8", "M".repeat(42))).unwrap(),
    )
    .unwrap();
    PgCheckout::new(pool.clone())
        .with_creator_posting(true)
        .checkout_once(&purchase)
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(1250)).await;
    let finance = PgCreatorFinance::new(pool.clone());
    let totals = finance.totals(owner).await.unwrap();
    assert_eq!(totals.wallet_balance_minor, 0);
    assert_eq!(totals.mature_minor, 200);
    let request = finance
        .reserve_once(
            owner,
            150,
            &IdempotencyKey::parse(&format!("{}8", "N".repeat(42))).unwrap(),
        )
        .await
        .unwrap()
        .id;

    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'administrator',true)",
    )
    .bind(admin)
    .execute(&pool)
    .await
    .unwrap();
    let mut session = [73_u8; 32];
    session[..8].copy_from_slice(&admin.to_be_bytes());
    sqlx::query(
        "INSERT INTO sessions(user_id,token_hash,expires_at)
         VALUES($1,$2,clock_timestamp()+interval '1 hour')",
    )
    .bind(admin)
    .bind(session.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    let reason = "Approve source-backed earnings";
    let digest = payout_step_up_payload_digest(admin, request, 150, reason);
    let token = "MMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMM";
    let hash = Sha256::digest(token.as_bytes());
    sqlx::query(
        "UPDATE sessions
         SET step_up_proof_hash=$2,step_up_scope='finance.payout.post.v1',
             step_up_resource_id=$3,step_up_revision=150,
             step_up_payload_digest=$4,
             step_up_issued_at=clock_timestamp()-interval '1 second',
             step_up_expires_at=clock_timestamp()+interval '4 minutes'
         WHERE token_hash=$1",
    )
    .bind(session.as_slice())
    .bind(hash.as_slice())
    .bind(request)
    .bind(digest.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    let repo = PgCreatorPayoutPosting::new(pool.clone());
    let result = repo
        .approve_once(admin, &session, token, request, 150, reason)
        .await
        .unwrap();
    assert!(!result.replayed);
    let (balance, credits, debits): (i64, i64, i64) = sqlx::query_as(
        "SELECT w.balance_minor,
           (SELECT count(*) FROM finance_creator_wallet_credits c
            WHERE c.user_id=w.user_id),
           (SELECT count(*) FROM reader_wallet_entries e
            WHERE e.user_id=w.user_id AND e.kind='creator_payout_debit')
         FROM reader_wallet_accounts w WHERE w.user_id=$1",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((balance, credits, debits), (50, 1, 1));
    assert_eq!(finance.totals(owner).await.unwrap().mature_minor, 0);
    assert_eq!(
        repo.approve_once(admin, &session, token, request, 150, reason)
            .await
            .unwrap()
            .wallet_entry_id,
        result.wallet_entry_id
    );
    sqlx::query(
        "UPDATE account_roles SET is_active=false WHERE user_id=$1 AND role='administrator'",
    )
    .bind(admin)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE sessions SET revoked_at=clock_timestamp() WHERE token_hash=$1")
        .bind(session.as_slice())
        .execute(&pool)
        .await
        .unwrap();
    pool.close().await;
}
