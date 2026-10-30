//! A-02/F-09: the account transaction freezes its referral decision.
//! Test-only direct SQL is confined to the owning PostgreSQL integration test.
use sqlx::PgPool;
use std::{
    num::NonZeroU32,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    repositories::{postgres::RegistrationReferral, PgAccounts},
    services::security::{generate_session_token, session_token_digest},
};

type Snapshot = (
    bool,           // signup decision captured
    Option<String>, // code
    Option<i64>,    // owner
    Option<String>, // program
    Option<i64>,    // capture
    Option<String>, // source
);

async fn pool() -> PgPool {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG URL");
    assert!(url.ends_with("/tor_ci"), "never use a production database");
    PgPool::connect(&url).await.expect("migrated PostgreSQL 18")
}

fn unique_email(tag: &str) -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("wall clock")
        .as_nanos();
    format!("f09-a-{tag}-{stamp}@example.invalid")
}

async fn seed_code(pool: &PgPool, program: &str) -> (i64, String) {
    let owner: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'F09 fixture','fixture-only') RETURNING id",
    )
    .bind(unique_email("owner"))
    .fetch_one(pool)
    .await
    .unwrap();
    let code = format!("f09a_{owner}");
    sqlx::query(
        "INSERT INTO referral_codes(code,owner_user_id,program_type)
         VALUES($1,$2,$3)",
    )
    .bind(&code)
    .bind(owner)
    .bind(program)
    .execute(pool)
    .await
    .unwrap();
    (owner, code)
}

async fn seed_capture(pool: &PgPool, guest: &[u8; 32], code: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO referral_captures(guest_hash,referral_code,source_kind,source_path)
         VALUES($1,$2,'direct_ref_link','/r/test') RETURNING id",
    )
    .bind(guest.as_slice())
    .bind(code)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn register(pool: &PgPool, guest: Option<&[u8; 32]>, manual: Option<&str>) -> i64 {
    let digest = session_token_digest(&generate_session_token());
    PgAccounts::from_pool(pool.clone())
        .register_with_session_referral(
            &unique_email("invited"),
            "F09 invited",
            "fixture-password-not-for-login",
            &digest,
            NonZeroU32::new(3600).unwrap(),
            RegistrationReferral {
                guest_hash: guest,
                manual_code: manual,
            },
        )
        .await
        .expect("account, session and referral decision commit atomically")
}

async fn snapshot(pool: &PgPool, user: i64) -> Snapshot {
    sqlx::query_as(
        "SELECT decided_at_registration,decided_referral_code,
                decided_referrer_user_id,decided_program_type,
                decided_capture_id,decided_source
         FROM account_registration_referral_outbox WHERE user_id=$1",
    )
    .bind(user)
    .fetch_one(pool)
    .await
    .expect("durable registration decision")
}

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_pg_signup_manual_owner_is_immutable_after_disable() {
    let db = pool().await;
    let (owner, code) = seed_code(&db, "reader_spend").await;
    let subject = register(&db, None, Some(&code)).await;
    let decided = snapshot(&db, subject).await;
    assert_eq!(
        decided,
        (
            true,
            Some(code.clone()),
            Some(owner),
            Some("reader_spend".into()),
            None,
            Some("manual_code_at_registration".into()),
        )
    );
    sqlx::query("UPDATE referral_codes SET status='disabled' WHERE code=$1")
        .bind(&code)
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(snapshot(&db, subject).await, decided);
}

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_pg_signup_guest_owner_survives_later_fraud_block() {
    let db = pool().await;
    let (owner, code) = seed_code(&db, "creator_income").await;
    let hash = session_token_digest(&generate_session_token());
    let capture_id = seed_capture(&db, &hash, &code).await;
    let subject = register(&db, Some(&hash), None).await;
    let decided = snapshot(&db, subject).await;
    assert_eq!(
        decided,
        (
            true,
            Some(code.clone()),
            Some(owner),
            Some("creator_income".into()),
            Some(capture_id),
            Some("guest_capture".into()),
        )
    );
    sqlx::query("UPDATE referral_codes SET status='blocked' WHERE code=$1")
        .bind(&code)
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("UPDATE referral_captures SET status='blocked' WHERE id=$1")
        .bind(capture_id)
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(snapshot(&db, subject).await, decided);
}

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_pg_signup_manual_priority_and_expired_code_fallback() {
    let db = pool().await;
    let (first_owner, first_code) = seed_code(&db, "reader_spend").await;
    let (second_owner, second_code) = seed_code(&db, "reader_spend").await;
    let hash = session_token_digest(&generate_session_token());
    let capture_id = seed_capture(&db, &hash, &first_code).await;

    let chosen = register(&db, Some(&hash), Some(&second_code)).await;
    let decided = snapshot(&db, chosen).await;
    assert_eq!(decided.2, Some(second_owner));
    assert_eq!(decided.4, None);
    assert_eq!(decided.5, Some("manual_code_at_registration".into()));

    sqlx::query("UPDATE referral_codes SET status='disabled' WHERE code=$1")
        .bind(&second_code)
        .execute(&db)
        .await
        .unwrap();
    let fallback = register(&db, Some(&hash), Some(&second_code)).await;
    let decided = snapshot(&db, fallback).await;
    assert_eq!(decided.2, Some(first_owner));
    assert_eq!(decided.4, Some(capture_id));
    assert_eq!(decided.5, Some("guest_capture".into()));
}

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_pg_signup_cannot_claim_later_click() {
    let db = pool().await;
    let (_, code) = seed_code(&db, "reader_spend").await;
    let hash = session_token_digest(&generate_session_token());
    let subject = register(&db, Some(&hash), None).await;
    assert_eq!(snapshot(&db, subject).await, (true, None, None, None, None, None));
    seed_capture(&db, &hash, &code).await;
    assert_eq!(snapshot(&db, subject).await, (true, None, None, None, None, None));
}
