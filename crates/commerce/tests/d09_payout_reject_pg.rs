//! S039/D-09: privileged payout rejection against disposable PostgreSQL only.
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

struct Fixture {
    pool: PgPool,
    applicant: i64,
    admin: i64,
    request: i64,
    digest: [u8; 32],
}

impl Fixture {
    /// The PostgreSQL runtime reuses tor_ci across suites. Never leave an
    /// active synthetic administrator behind: A-04/A-05 validates that a real
    /// last administrator cannot be removed.
    async fn finish(self) {
        let changed = sqlx::query(
            "UPDATE account_roles SET is_active=false
             WHERE user_id=$1 AND role='administrator'",
        )
        .bind(self.admin)
        .execute(&self.pool)
        .await
        .expect("S039 synthetic administrator cleanup")
        .rows_affected();
        assert_eq!(changed, 1, "S039 fixture administrator role must exist");
        let active: bool = sqlx::query_scalar(
            "SELECT is_active FROM account_roles
             WHERE user_id=$1 AND role='administrator'",
        )
        .bind(self.admin)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        assert!(!active, "never pollute subsequent A-04 last-admin tests");
        // Preserve the audit and request rows; only revoke the synthetic role.
        self.finish().await;
    }
}

async fn fixture(scenario: &str) -> Fixture {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PostgreSQL tor_ci required");
    assert!(
        url.ends_with("/tor_ci"),
        "never operate against retained PostgreSQL"
    );
    let pool = PgPool::connect(&url).await.unwrap();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let applicant: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Payout applicant','$argon2id$ci') RETURNING id",
    )
    .bind(format!("s039-owner-{scenario}-{stamp}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let admin: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Payout administrator','$argon2id$ci') RETURNING id",
    )
    .bind(format!("s039-admin-{scenario}-{stamp}@example.test"))
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO books(source_identity,target_language,title,visibility,owner_user_id)
         VALUES($1,'ru','S039 fixture','public',$2)",
    )
    .bind(format!("ci:s039:{scenario}:{stamp}"))
    .bind(applicant)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reader_wallet_accounts(user_id,balance_minor,currency)
         VALUES($1,1000,'RUB')",
    )
    .bind(applicant)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'administrator',true)
         ON CONFLICT (user_id,role) DO UPDATE SET is_active=true",
    )
    .bind(admin)
    .execute(&pool)
    .await
    .unwrap();
    let digest = [91_u8; 32];
    // Use a different digest for each fixture, so parallel tests cannot reuse
    // a session or accidentally revoke another test's administrator.
    let mut digest = digest;
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
    let key = IdempotencyKey::parse(&format!("{}8", "A".repeat(42))).unwrap();
    let request = PgCreatorFinance::new(pool.clone())
        .reserve_once(applicant, 600, &key)
        .await
        .unwrap()
        .id;
    Fixture {
        pool,
        applicant,
        admin,
        request,
        digest,
    }
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_09_reject_exactly_once_releases_reservation_without_wallet_debit() {
    let f = fixture("one_rejection").await;
    let repo = PgCreatorPayoutAdmin::new(f.pool.clone());
    let finance = PgCreatorFinance::new(f.pool.clone());
    assert_eq!(
        finance.totals(f.applicant).await.unwrap().reserved_minor,
        600
    );
    assert_eq!(
        finance.totals(f.applicant).await.unwrap().available_minor,
        400
    );
    let shown = repo.list(&f.digest, None).await.unwrap();
    assert!(shown
        .iter()
        .any(|p| p.id == f.request && p.amount_minor == 600 && p.state == "pending"));
    let (first, second) = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        tokio::join!(
            repo.reject_once(&f.digest, f.request, "Request rejected"),
            repo.reject_once(&f.digest, f.request, "Request rejected")
        )
    })
    .await
    .expect("S039 reject deadlock");
    let (first, second) = (first.unwrap(), second.unwrap());
    assert_eq!(first.replayed as u8 + second.replayed as u8, 1);
    assert_eq!(
        repo.reject_once(&f.digest, f.request, "Different reason")
            .await,
        Err(PayoutAdminError::Conflict)
    );
    let (state, decision_actor, reason): (String, Option<i64>, Option<String>) = sqlx::query_as(
        "SELECT state,decision_actor_id,decision_reason
         FROM finance_creator_payout_requests WHERE id=$1",
    )
    .bind(f.request)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(state, "rejected");
    assert_eq!(decision_actor, Some(f.admin));
    assert_eq!(reason.as_deref(), Some("Request rejected"));
    let audit_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM finance_creator_payout_admin_audit
         WHERE payout_request_id=$1 AND action='reject'",
    )
    .bind(f.request)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(audit_count, 1);
    let wallet: (i64,) =
        sqlx::query_as("SELECT balance_minor FROM reader_wallet_accounts WHERE user_id=$1")
            .bind(f.applicant)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(wallet.0, 1000);
    let journal: i64 =
        sqlx::query_scalar("SELECT count(*) FROM reader_wallet_entries WHERE user_id=$1")
            .bind(f.applicant)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(journal, 0, "rejection is not a wallet movement");
    assert_eq!(finance.totals(f.applicant).await.unwrap().reserved_minor, 0);
    assert_eq!(
        finance.totals(f.applicant).await.unwrap().available_minor,
        1000
    );
    assert!(sqlx::query(
        "UPDATE finance_creator_payout_admin_audit SET action='list_requests'
         WHERE payout_request_id=$1",
    )
    .bind(f.request)
    .execute(&f.pool)
    .await
    .is_err());
    assert!(sqlx::query(
        "DELETE FROM finance_creator_payout_admin_audit WHERE payout_request_id=$1",
    )
    .bind(f.request)
    .execute(&f.pool)
    .await
    .is_err());
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_09_reject_denies_non_admin_expired_and_revoked_sessions() {
    let f = fixture("denied").await;
    let repo = PgCreatorPayoutAdmin::new(f.pool.clone());
    let denied = [0_u8; 32];
    assert_eq!(
        repo.list(&denied, None).await,
        Err(PayoutAdminError::NotFound)
    );
    assert_eq!(
        repo.reject_once(&denied, f.request, "No authority").await,
        Err(PayoutAdminError::NotFound)
    );
    sqlx::query(
        "UPDATE account_roles SET is_active=false
         WHERE user_id=$1 AND role='administrator'",
    )
    .bind(f.admin)
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        repo.reject_once(&f.digest, f.request, "Role revoked").await,
        Err(PayoutAdminError::NotFound)
    );
    sqlx::query(
        "UPDATE account_roles SET is_active=true
         WHERE user_id=$1 AND role='administrator'",
    )
    .bind(f.admin)
    .execute(&f.pool)
    .await
    .unwrap();
    sqlx::query("UPDATE sessions SET revoked_at=clock_timestamp() WHERE token_hash=$1")
        .bind(f.digest.as_slice())
        .execute(&f.pool)
        .await
        .unwrap();
    assert_eq!(
        repo.reject_once(&f.digest, f.request, "Session revoked")
            .await,
        Err(PayoutAdminError::NotFound)
    );
    let pending: String =
        sqlx::query_scalar("SELECT state FROM finance_creator_payout_requests WHERE id=$1")
            .bind(f.request)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(pending, "pending");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM finance_creator_payout_admin_audit
         WHERE payout_request_id=$1",
    )
    .bind(f.request)
    .fetch_one(&f.pool)
    .await
    .unwrap();
    assert_eq!(count, 0);
    f.finish().await;
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn d_09_reject_audit_failure_rolls_back_terminal_state() {
    let f = fixture("rollback").await;
    let repo = PgCreatorPayoutAdmin::new(f.pool.clone());
    sqlx::query(
        "INSERT INTO finance_creator_payout_admin_audit
         (admin_user_id,payout_request_id,action) VALUES($1,$2,'reject')",
    )
    .bind(f.admin)
    .bind(f.request)
    .execute(&f.pool)
    .await
    .unwrap();
    assert_eq!(
        repo.reject_once(&f.digest, f.request, "Must rollback")
            .await,
        Err(PayoutAdminError::Unavailable)
    );
    let state: String =
        sqlx::query_scalar("SELECT state FROM finance_creator_payout_requests WHERE id=$1")
            .bind(f.request)
            .fetch_one(&f.pool)
            .await
            .unwrap();
    assert_eq!(
        state, "pending",
        "an audit failure cannot release the reservation"
    );
    let finance = PgCreatorFinance::new(f.pool.clone());
    assert_eq!(
        finance.totals(f.applicant).await.unwrap().reserved_minor,
        600
    );
    f.finish().await;
}
