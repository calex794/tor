//! F-09/S052: live PostgreSQL 18 acceptance of first touch, replay and recovery.
//! Every ignored case is explicitly run by TOR's isolated PostgreSQL CI.
use sqlx::PgPool;
use tor_referrals::{
    domain::{manual_code, valid_code},
    repositories::{postgres::CaptureResult, PgReferrals},
};

#[test]
fn f_09_invalid_manual_code_is_not_silently_truncated() {
    assert!(manual_code(Some("not a code")).is_err());
    assert!(manual_code(Some("valid_123")).unwrap().is_some());
    assert!(!valid_code("x"));
}

async fn isolated_pool() -> PgPool {
    let url = std::env::var("TEST_DATABASE_URL").expect("test PostgreSQL");
    assert!(
        url.ends_with("/tor_ci"),
        "F-09 integration may only use disposable tor_ci"
    );
    PgPool::connect(&url).await.expect("connect to migrated PG18")
}

async fn seed_user(pool: &PgPool, role: &str) -> i64 {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("wall clock")
        .as_nanos();
    let email = format!("f09-{role}-{stamp}@example.invalid");
    sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash) \
         VALUES($1,'F09 fixture','not-a-real-password') RETURNING id",
    )
    .bind(email)
    .fetch_one(pool)
    .await
    .expect("seed F-09 user")
}

fn guest_hash(seed: i64) -> [u8; 32] {
    let mut value = [0_u8; 32];
    value[..8].copy_from_slice(&seed.to_be_bytes());
    value[8..16].copy_from_slice(&(!seed).to_be_bytes());
    value
}

async fn seed_code(pool: &PgPool, owner: i64, program: &str) -> String {
    let code = format!("f09_{owner}");
    sqlx::query(
        "INSERT INTO referral_codes(code,owner_user_id,program_type) VALUES($1,$2,$3)",
    )
    .bind(&code)
    .bind(owner)
    .bind(program)
    .execute(pool)
    .await
    .expect("seed F-09 code");
    code
}

async fn enqueue(
    pool: &PgPool,
    user: i64,
    guest: Option<&[u8; 32]>,
    manual: Option<&str>,
) {
    sqlx::query(
        "INSERT INTO account_registration_referral_outbox(user_id,guest_hash,manual_code) \
         VALUES($1,$2,$3)",
    )
    .bind(user)
    .bind(guest.map(|hash| hash.as_slice()))
    .bind(manual)
    .execute(pool)
    .await
    .expect("durable F-09 registration event");
}

async fn attribution_count(pool: &PgPool, subject: i64) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM referral_attributions WHERE subject_user_id=$1")
        .bind(subject)
        .fetch_one(pool)
        .await
        .expect("attribution count")
}

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_pg_first_capture_and_replay() {
    let pool = isolated_pool().await;
    let repo = PgReferrals::new(pool.clone());
    let first = seed_user(&pool, "first").await;
    let second = seed_user(&pool, "second").await;
    let first_code = seed_code(&pool, first, "reader_spend").await;
    let second_code = seed_code(&pool, second, "reader_spend").await;
    let guest_hash = guest_hash(first);
    assert_eq!(
        repo.capture(&guest_hash, &first_code, "direct_ref_link", "/r/first")
            .await
            .unwrap(),
        CaptureResult::Saved
    );
    assert_eq!(
        repo.capture(&guest_hash, &second_code, "direct_ref_link", "/r/second")
            .await
            .unwrap(),
        CaptureResult::Existing,
        "second click must not steal first touch"
    );
    let invited = seed_user(&pool, "invited").await;
    enqueue(&pool, invited, Some(&guest_hash), None).await;
    assert!(repo.finalize_one(invited).await.unwrap());
    assert!(!repo.finalize_one(invited).await.unwrap());
    assert_eq!(attribution_count(&pool, invited).await, 1);
    let referrer: i64 = sqlx::query_scalar(
        "SELECT referrer_user_id FROM referral_attributions WHERE subject_user_id=$1",
    )
    .bind(invited)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(referrer, first);
}

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_pg_manual_precedence_invalid_fallback_and_self_invite() {
    let pool = isolated_pool().await;
    let repo = PgReferrals::new(pool.clone());
    let first = seed_user(&pool, "manual-first").await;
    let second = seed_user(&pool, "manual-second").await;
    let first_code = seed_code(&pool, first, "reader_spend").await;
    let second_code = seed_code(&pool, second, "reader_spend").await;
    let guest_hash = guest_hash(first);
    assert_eq!(
        repo.capture(&guest_hash, &first_code, "direct_ref_link", "/r/first")
            .await
            .unwrap(),
        CaptureResult::Saved
    );

    let priority = seed_user(&pool, "manual-priority").await;
    enqueue(&pool, priority, Some(&guest_hash), Some(&second_code)).await;
    assert!(repo.finalize_one(priority).await.unwrap());
    let winner: i64 = sqlx::query_scalar(
        "SELECT referrer_user_id FROM referral_attributions WHERE subject_user_id=$1",
    )
    .bind(priority)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(winner, second, "valid manual code outranks guest capture");

    let fallback = seed_user(&pool, "manual-fallback").await;
    enqueue(
        &pool,
        fallback,
        Some(&guest_hash),
        Some("missing_invite_123456"),
    )
    .await;
    assert!(repo.finalize_one(fallback).await.unwrap());
    let fallback_referrer: i64 = sqlx::query_scalar(
        "SELECT referrer_user_id FROM referral_attributions WHERE subject_user_id=$1",
    )
    .bind(fallback)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(fallback_referrer, first, "unknown manual code retains guest capture");

    enqueue(&pool, first, None, Some(&first_code)).await;
    assert!(!repo.finalize_one(first).await.unwrap());
    assert_eq!(attribution_count(&pool, first).await, 0);
}

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_pg_recovery_uses_signup_time_not_replay_time() {
    let pool = isolated_pool().await;
    let repo = PgReferrals::new(pool.clone());
    let owner = seed_user(&pool, "recovery-owner").await;
    let code = seed_code(&pool, owner, "reader_spend").await;
    let guest_hash = guest_hash(owner);
    assert_eq!(
        repo.capture(&guest_hash, &code, "direct_ref_link", "/r/recovery")
            .await
            .unwrap(),
        CaptureResult::Saved
    );
    // Reproduce a registered user pending after a five-day processing outage.
    // The invitation was valid at signup but has expired by the time of replay.
    sqlx::query(
        "UPDATE referral_captures SET captured_at=now()-INTERVAL '35 days', \
         expires_at=now()-INTERVAL '5 days' WHERE guest_hash=$1",
    )
    .bind(guest_hash.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    let invited = seed_user(&pool, "recovery-invited").await;
    enqueue(&pool, invited, Some(&guest_hash), None).await;
    sqlx::query(
        "UPDATE account_registration_referral_outbox \
         SET registered_at=now()-INTERVAL '6 days' WHERE user_id=$1",
    )
    .bind(invited)
    .execute(&pool)
    .await
    .unwrap();

    // This is the same public API called by the periodic recovery consumer.
    assert!(repo.finalize_pending(50).await.unwrap() >= 1);
    assert_eq!(attribution_count(&pool, invited).await, 1);
    assert!(!repo.finalize_one(invited).await.unwrap());
    let result: String = sqlx::query_scalar(
        "SELECT result FROM account_registration_referral_outbox WHERE user_id=$1",
    )
    .bind(invited)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(result, "attributed");

    // An invitation captured AFTER a signup must never retroactively attach.
    let late_guest = guest_hash(invited);
    let before_click = seed_user(&pool, "before-click").await;
    enqueue(&pool, before_click, Some(&late_guest), None).await;
    sqlx::query(
        "UPDATE account_registration_referral_outbox \
         SET registered_at=now()-INTERVAL '1 day' WHERE user_id=$1",
    )
    .bind(before_click)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        repo.capture(&late_guest, &code, "direct_ref_link", "/r/late")
            .await
            .unwrap(),
        CaptureResult::Saved
    );
    assert!(!repo.finalize_one(before_click).await.unwrap());
    assert_eq!(attribution_count(&pool, before_click).await, 0);
}

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_pg_concurrent_replay_single_attribution() {
    let pool = isolated_pool().await;
    let repo = PgReferrals::new(pool.clone());
    let owner = seed_user(&pool, "race-owner").await;
    let code = seed_code(&pool, owner, "reader_spend").await;
    let invited = seed_user(&pool, "race-invited").await;
    enqueue(&pool, invited, None, Some(&code)).await;
    let first = repo.clone();
    let second = repo.clone();
    let (a, b) = tokio::join!(
        async move { first.finalize_one(invited).await.unwrap() },
        async move { second.finalize_one(invited).await.unwrap() }
    );
    assert_ne!(a, b, "only the first processor can claim a registration");
    assert_eq!(attribution_count(&pool, invited).await, 1);
    let pending: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_registration_referral_outbox \
         WHERE user_id=$1 AND processed_at IS NULL",
    )
    .bind(invited)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(pending, 0);
}
