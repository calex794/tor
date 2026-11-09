//! S039/MONEY-03: owner-lock approval quote, NOT a payout.
#![forbid(unsafe_code)]
use sqlx::PgPool;
use std::time::{SystemTime, UNIX_EPOCH};
use tor_commerce::{
    domain::money::IdempotencyKey,
    repositories::{
        creator_finance::PgCreatorFinance,
        creator_payout_admin::{PayoutAdminError, PgCreatorPayoutAdmin},
    },
};

struct Setup {
    pool: PgPool,
    owner: i64,
    admin: i64,
    digest: [u8; 32],
    q1: i64,
    q2: i64,
}

async fn setup(tag: &str) -> Setup {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable tor_ci required");
    assert!(
        url.ends_with("/tor_ci"),
        "never operate against retained DB"
    );
    let pool = PgPool::connect(&url).await.unwrap();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Quote author','$argon2id$ci') RETURNING id",
    )
    .bind(format!("s039-quote-owner-{tag}-{stamp}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let admin: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Quote admin','$argon2id$ci') RETURNING id",
    )
    .bind(format!("s039-quote-admin-{tag}-{stamp}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','S039 approval quote','public',$2)",
    )
    .bind(format!("ci:s039:approval:{tag}:{stamp}"))
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES($1,1000,'RUB')",
    )
    .bind(owner)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'administrator',true)",
    )
    .bind(admin)
    .execute(&pool)
    .await
    .unwrap();
    let mut digest = [79_u8; 32];
    digest[..8].copy_from_slice(&admin.to_be_bytes());
    sqlx::query(
        "INSERT INTO sessions(user_id,token_hash,expires_at)
         VALUES($1,$2,clock_timestamp()+interval '1 hour')",
    )
    .bind(admin)
    .bind(digest.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    let repo = PgCreatorFinance::new(pool.clone());
    let q1 = repo
        .reserve_once(
            owner,
            600,
            &IdempotencyKey::parse(&format!("{}8", "A".repeat(42))).unwrap(),
        )
        .await
        .unwrap()
        .id;
    let q2 = repo
        .reserve_once(
            owner,
            300,
            &IdempotencyKey::parse(&format!("{}8", "B".repeat(42))).unwrap(),
        )
        .await
        .unwrap()
        .id;
    Setup {
        pool,
        owner,
        admin,
        digest,
        q1,
        q2,
    }
}

impl Setup {
    async fn finish(self) {
        sqlx::query(
            "UPDATE account_roles SET is_active=false
             WHERE user_id=$1 AND role='administrator'",
        )
        .bind(self.admin)
        .execute(&self.pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE sessions SET revoked_at=COALESCE(revoked_at,clock_timestamp())
             WHERE token_hash=$1",
        )
        .bind(self.digest.as_slice())
        .execute(&self.pool)
        .await
        .unwrap();
        self.pool.close().await;
    }
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_09_quote_excludes_own_reservation_and_does_not_debit() {
    let f = setup("two-pending").await;
    let repo = PgCreatorPayoutAdmin::new(f.pool.clone());
    let first = repo.approval_readiness(&f.digest, f.q1).await.unwrap();
    assert_eq!(first.user_id, f.owner);
    assert_eq!(first.amount_minor, 600);
    assert_eq!(first.other_reserved_minor, 300);
    assert_eq!(first.available_excluding_own_minor, 700);
    assert!(first.sufficient);
    let second = repo.approval_readiness(&f.digest, f.q2).await.unwrap();
    assert_eq!(second.amount_minor, 300);
    assert_eq!(second.other_reserved_minor, 600);
    assert_eq!(second.available_excluding_own_minor, 400);
    assert!(second.sufficient);
    let balance: i64 =
        sqlx::query_scalar("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(f.owner)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(balance, 1000);
    let journal: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reader_wallet_entries WHERE user_id=$1")
            .bind(f.owner)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(journal, 0, "quote is not a wallet movement");
    let audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM finance_creator_payout_approval_read_audit
         WHERE admin_user_id=$1 AND payout_request_id IN ($2,$3)",
    )
    .bind(f.admin)
    .bind(f.q1)
    .bind(f.q2)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(audits, 2);
    assert!(sqlx::query(
        "DELETE FROM finance_creator_payout_approval_read_audit
         WHERE payout_request_id=$1",
    )
    .bind(f.q1)
    .execute(&f.pool)
    .await
    .is_err());
    assert_eq!(
        repo.approval_readiness(&[0_u8; 32], f.q1).await,
        Err(PayoutAdminError::NotFound)
    );
    repo.reject_once(&f.digest, f.q1, "Reject first")
        .await
        .unwrap();
    assert_eq!(
        repo.approval_readiness(&f.digest, f.q1).await,
        Err(PayoutAdminError::Conflict)
    );
    assert_eq!(
        repo.approval_readiness(&f.digest, f.q2)
            .await
            .unwrap()
            .available_excluding_own_minor,
        1000
    );
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_09_quote_clamps_signed_deficit_before_eligibility_and_respects_revocation() {
    let f = setup("signed-deficit").await;
    let repo = PgCreatorPayoutAdmin::new(f.pool.clone());
    // Only this disposable migrator fixture simulates a new obligation that
    // makes an earlier reservation underfunded. The app cannot bypass
    // reserve_once: approval must recheck the state at decision time.
    sqlx::query(
        "INSERT INTO finance_creator_payout_requests
         (user_id,amount_minor,intent_key,intent_digest,reservation_minor)
         VALUES($1,900,$2,decode(repeat('e',64),'hex'),900)",
    )
    .bind(f.owner)
    .bind(format!("{}8", "C".repeat(42)))
    .execute(&f.pool)
    .await
    .unwrap();
    let quote = repo.approval_readiness(&f.digest, f.q1).await.unwrap();
    assert_eq!(quote.other_reserved_minor, 1200);
    assert_eq!(quote.available_excluding_own_minor, 0);
    assert!(
        !quote.sufficient,
        "must not add own reserve after clamping debt"
    );
    sqlx::query("UPDATE sessions SET revoked_at=clock_timestamp() WHERE token_hash=$1")
        .bind(f.digest.as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        repo.approval_readiness(&f.digest, f.q1).await,
        Err(PayoutAdminError::NotFound)
    );
    f.finish().await;
}
