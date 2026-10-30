//! F-09: signup-time referral decisions must survive delayed outbox replay.
//! These cases use the real account transaction and a disposable PostgreSQL 18.
use std::{num::NonZeroU32, time::{SystemTime, UNIX_EPOCH}};
use sqlx::PgPool;
use tor_accounts::{
    repositories::{postgres::RegistrationReferral, PgAccounts},
    services::security::{generate_session_token, session_token_digest},
};
use tor_referrals::repositories::{postgres::CaptureResult, PgReferrals};

async fn test_pool() -> PgPool {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.ends_with("/tor_ci"), "never test against production");
    PgPool::connect(&url).await.expect("migrated PostgreSQL 18")
}

fn unique_email(role: &str) -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("wall clock")
        .as_nanos();
    format!("f09-snapshot-{role}-{stamp}@example.invalid")
}

async fn inviter(pool: &PgPool, role: &str, program: &str) -> (i64, String) {
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'F09 snapshot inviter','test-only') RETURNING id",
    )
    .bind(unique_email(role))
    .fetch_one(pool)
    .await
    .expect("seed inviter");
    let code = format!("f09s_{owner}");
    sqlx::query(
        "INSERT INTO referral_codes(code,owner_user_id,program_type)
         VALUES($1,$2,$3)",
    )
    .bind(&code)
    .bind(owner)
    .bind(program)
    .execute(pool)
    .await
    .expect("seed referral code");
    (owner, code)
}

async fn register(
    accounts: &PgAccounts,
    role: &str,
    guest: Option<&[u8; 32]>,
    manual: Option<&str>,
) -> i64 {
    let digest = session_token_digest(&generate_session_token());
    accounts
        .register_with_session_referral(
            &unique_email(role),
            "F09 invited",
            "fixture-password-not-for-login",
            &digest,
            NonZeroU32::new(3600).expect("session ttl"),
            RegistrationReferral {
                guest_hash: guest,
                manual_code: manual,
            },
        )
        .await
        .expect("signup, first session and decided outbox commit")
}

async fn asserted_referrer(pool: &PgPool, subject: i64) -> (i64, String) {
    sqlx::query_as(
        "SELECT referrer_user_id,attribution_source
         FROM referral_attributions WHERE subject_user_id=$1",
    )
    .bind(subject)
    .fetch_one(pool)
    .await
    .expect("signup-time attribution")
}

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_snapshot_manual_survives_disabled_code() {
    let pool = test_pool().await;
    let (owner, code) = inviter(&pool, "manual", "reader_spend").await;
    let subject = register(&PgAccounts::from_pool(pool.clone()), "manual-user", None, Some(&code)).await;
    let saved: (bool, String, i64, String) = sqlx::query_as(
        "SELECT decided_at_registration,decided_referral_code,
                decided_referrer_user_id,decided_source
         FROM account_registration_referral_outbox WHERE user_id=$1",
    )
    .bind(subject)
    .fetch_one(&pool)
    .await
    .expect("durable manual decision");
    assert_eq!(saved, (true, code.clone(), owner, "manual_code_at_registration".into()));

    // The signup already committed. Disabling the code must not silently
    // delete its previously eligible owner from a delayed outbox replay.
    sqlx::query("UPDATE referral_codes SET status='disabled' WHERE code=$1")
        .bind(&code)
        .execute(&pool)
        .await
        .unwrap();
    let repo = PgReferrals::new(pool.clone());
    assert!(repo.finalize_one(subject).await.unwrap());
    assert_eq!(
        asserted_referrer(&pool, subject).await,
        (owner, "manual_code_at_registration".into())
    );
    let status: String = sqlx::query_scalar(
        "SELECT status FROM referral_attributions WHERE subject_user_id=$1",
    )
    .bind(subject)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "active", "later disabling must not erase earned attribution");
    assert!(!repo.finalize_one(subject).await.unwrap());
}

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_snapshot_guest_survives_blocked_code_and_capture() {
    let pool = test_pool().await;
    let (owner, code) = inviter(&pool, "guest", "creator_income").await;
    let hash = session_token_digest(&generate_session_token());
    let repo = PgReferrals::new(pool.clone());
    assert_eq!(
        repo.capture(&hash, &code, "direct_ref_link", "/r/invite")
            .await
            .unwrap(),
        CaptureResult::Saved,
    );
    let subject = register(&PgAccounts::from_pool(pool.clone()), "guest-user", Some(&hash), None).await;
    let saved: (bool, String, String, i64) = sqlx::query_as(
        "SELECT decided_at_registration,decided_referral_code,
                decided_source,decided_capture_id
         FROM account_registration_referral_outbox WHERE user_id=$1",
    )
    .bind(subject)
    .fetch_one(&pool)
    .await
    .expect("durable guest decision");
    assert_eq!(saved.0, true);
    assert_eq!(saved.1, code);
    assert_eq!(saved.2, "guest_capture");
    assert!(saved.3 > 0);

    sqlx::query("UPDATE referral_codes SET status='blocked' WHERE code=$1")
        .bind(&code)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE referral_captures SET status='blocked' WHERE id=$1")
        .bind(saved.3)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo.finalize_one(subject).await.unwrap());
    assert_eq!(
        asserted_referrer(&pool, subject).await,
        (owner, "guest_capture".into())
    );
    let status: String = sqlx::query_scalar(
        "SELECT status FROM referral_attributions WHERE subject_user_id=$1",
    )
    .bind(subject)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(status, "blocked", "fraud flag must prevent a live referral");
    let valid_expiry: bool = sqlx::query_scalar(
        "SELECT a.attributed_at = o.registered_at
            AND a.expires_at = o.registered_at + INTERVAL '365 days'
         FROM referral_attributions a
         JOIN account_registration_referral_outbox o ON o.user_id=a.subject_user_id
         WHERE a.subject_user_id=$1",
    )
    .bind(subject)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(valid_expiry, "creator expiry uses registration time");
}

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_snapshot_rejects_click_after_account_creation() {
    let pool = test_pool().await;
    let (owner, code) = inviter(&pool, "late", "reader_spend").await;
    let hash = session_token_digest(&generate_session_token());
    let repo = PgReferrals::new(pool.clone());
    let subject = register(&PgAccounts::from_pool(pool.clone()), "pre-click", Some(&hash), None).await;
    let stored: (bool, Option<String>) = sqlx::query_as(
        "SELECT decided_at_registration,decided_referral_code
         FROM account_registration_referral_outbox WHERE user_id=$1",
    )
    .bind(subject)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored, (true, None));

    assert_eq!(
        repo.capture(&hash, &code, "direct_ref_link", "/r/late")
            .await
            .unwrap(),
        CaptureResult::Saved
    );
    assert!(!repo.finalize_one(subject).await.unwrap());
    let attributed: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM referral_attributions WHERE subject_user_id=$1",
    )
    .bind(subject)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(attributed, 0, "later click must not claim this signup");
    let result: String = sqlx::query_scalar(
        "SELECT result FROM account_registration_referral_outbox WHERE user_id=$1",
    )
    .bind(subject)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(result, "unattributed");
    assert!(owner > 0);
}
