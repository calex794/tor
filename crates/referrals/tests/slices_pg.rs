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
    PgPool::connect(&url)
        .await
        .expect("connect to migrated PG18")
}

async fn seed_user(pool: &PgPool, role: &str) -> i64 {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("wall clock")
        .as_nanos();
    let email = format!("f09-{role}-{stamp}@example.invalid");
    sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash) \
         VALUES($1,'F09 fixture','$argon2id$test') RETURNING id",
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
    sqlx::query("INSERT INTO referral_codes(code,owner_user_id,program_type) VALUES($1,$2,$3)")
        .bind(&code)
        .bind(owner)
        .bind(program)
        .execute(pool)
        .await
        .expect("seed F-09 code");
    code
}

async fn enqueue(pool: &PgPool, user: i64, guest: Option<&[u8; 32]>, manual: Option<&str>) {
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
async fn f_09_pg_manual_codes_must_be_active() {
    let pool = isolated_pool().await;
    let repo = PgReferrals::new(pool.clone());
    let inviter = seed_user(&pool, "invite-status").await;
    let code = seed_code(&pool, inviter, "reader_spend").await;
    assert!(repo.active_code(&code).await.unwrap());
    assert!(!repo.active_code("missing_referral_2026").await.unwrap());

    sqlx::query("UPDATE referral_codes SET status='disabled' WHERE code=$1")
        .bind(&code)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!repo.active_code(&code).await.unwrap());
    assert_eq!(
        repo.capture(
            &guest_hash(inviter),
            &code,
            "direct_ref_link",
            "/r/disabled"
        )
        .await
        .unwrap(),
        CaptureResult::Invalid,
    );

    sqlx::query("UPDATE referral_codes SET status='blocked' WHERE code=$1")
        .bind(&code)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!repo.active_code(&code).await.unwrap());
}

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_pg_decided_manual_survives_disabled_code() {
    let pool = isolated_pool().await;
    let repo = PgReferrals::new(pool.clone());
    let inviter = seed_user(&pool, "decided-manual-owner").await;
    let subject = seed_user(&pool, "decided-manual-subject").await;
    let code = seed_code(&pool, inviter, "reader_spend").await;
    sqlx::query(
        "INSERT INTO account_registration_referral_outbox(
            user_id,manual_code,decided_at_registration,
            decided_referral_code,decided_referrer_user_id,
            decided_program_type,decided_source)
         VALUES($1,$2,TRUE,$2,$3,'reader_spend','manual_code_at_registration')",
    )
    .bind(subject)
    .bind(&code)
    .bind(inviter)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE referral_codes SET status='disabled' WHERE code=$1")
        .bind(&code)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo.finalize_one(subject).await.unwrap());
    let (actual, status): (i64, String) = sqlx::query_as(
        "SELECT referrer_user_id,status FROM referral_attributions WHERE subject_user_id=$1",
    )
    .bind(subject)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(actual, inviter);
    assert_eq!(status, "active");
    assert!(!repo.finalize_one(subject).await.unwrap());
}

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_pg_decided_guest_keeps_owner_but_blocks_fraud() {
    let pool = isolated_pool().await;
    let repo = PgReferrals::new(pool.clone());
    let inviter = seed_user(&pool, "decided-guest-owner").await;
    let subject = seed_user(&pool, "decided-guest-subject").await;
    let code = seed_code(&pool, inviter, "creator_income").await;
    let guest = guest_hash(inviter);
    assert_eq!(
        repo.capture(&guest, &code, "direct_ref_link", "/r/snapshot")
            .await
            .unwrap(),
        CaptureResult::Saved
    );
    let capture_id: i64 = sqlx::query_scalar(
        "SELECT id FROM referral_captures WHERE guest_hash=$1 AND status='active'",
    )
    .bind(guest.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO account_registration_referral_outbox(
            user_id,guest_hash,decided_at_registration,
            decided_referral_code,decided_referrer_user_id,decided_program_type,
            decided_capture_id,decided_source)
         VALUES($1,$2,TRUE,$3,$4,'creator_income',$5,'guest_capture')",
    )
    .bind(subject)
    .bind(guest.as_slice())
    .bind(&code)
    .bind(inviter)
    .bind(capture_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE referral_codes SET status='blocked' WHERE code=$1")
        .bind(&code)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE referral_captures SET status='blocked' WHERE id=$1")
        .bind(capture_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo.finalize_one(subject).await.unwrap());
    let (actual, status, source): (i64, String, String) = sqlx::query_as(
        "SELECT referrer_user_id,status,attribution_source
         FROM referral_attributions WHERE subject_user_id=$1",
    )
    .bind(subject)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(actual, inviter);
    assert_eq!(
        status, "blocked",
        "fraud block must not create active rights"
    );
    assert_eq!(source, "guest_capture");
    assert!(!repo.finalize_one(subject).await.unwrap());
}

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_pg_decided_empty_cannot_retroactively_claim_signup() {
    let pool = isolated_pool().await;
    let repo = PgReferrals::new(pool.clone());
    let inviter = seed_user(&pool, "empty-owner").await;
    let subject = seed_user(&pool, "empty-subject").await;
    let code = seed_code(&pool, inviter, "reader_spend").await;
    let guest = guest_hash(subject);
    sqlx::query(
        "INSERT INTO account_registration_referral_outbox(
            user_id,guest_hash,decided_at_registration) VALUES($1,$2,TRUE)",
    )
    .bind(subject)
    .bind(guest.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        repo.capture(&guest, &code, "direct_ref_link", "/r/too-late")
            .await
            .unwrap(),
        CaptureResult::Saved
    );
    assert!(!repo.finalize_one(subject).await.unwrap());
    assert_eq!(attribution_count(&pool, subject).await, 0);
    let result: String = sqlx::query_scalar(
        "SELECT result FROM account_registration_referral_outbox WHERE user_id=$1",
    )
    .bind(subject)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(result, "unattributed");
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
    assert_eq!(
        fallback_referrer, first,
        "unknown manual code retains guest capture"
    );

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
    // The guest visits a different code after the prior invitation expired.
    // Recovery must still select the historical inviter valid at registration.
    sqlx::query("UPDATE referral_captures SET status='expired' WHERE guest_hash=$1")
        .bind(guest_hash.as_slice())
        .execute(&pool)
        .await
        .unwrap();
    let later_owner = seed_user(&pool, "later-owner").await;
    let later_code = seed_code(&pool, later_owner, "reader_spend").await;
    assert_eq!(
        repo.capture(&guest_hash, &later_code, "direct_ref_link", "/r/later")
            .await
            .unwrap(),
        CaptureResult::Saved
    );

    // This is the same public API called by the periodic recovery consumer.
    assert!(repo.finalize_pending(50).await.unwrap() >= 1);
    assert_eq!(attribution_count(&pool, invited).await, 1);
    let row_correct: bool = sqlx::query_scalar(
        "SELECT a.referrer_user_id = $2 AND a.attributed_at = o.registered_at \
         FROM referral_attributions a \
         JOIN account_registration_referral_outbox o ON o.user_id=a.subject_user_id \
         WHERE a.subject_user_id=$1",
    )
    .bind(invited)
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(
        row_correct,
        "original inviter and signup timestamp must survive replay"
    );
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
    let late_guest = crate::guest_hash(invited);
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

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_pg_self_service_code_is_idempotent_and_moderation_safe() {
    let pool = isolated_pool().await;
    let repo = PgReferrals::new(pool.clone());
    let owner = seed_user(&pool, "self-service").await;
    let initial = format!("r_{}_first", owner);
    let competing = format!("r_{}_other", owner);
    assert!(repo.default_reader_code(owner).await.unwrap().is_none());
    let (first, second) = tokio::join!(
        repo.create_default_reader_code(owner, &initial),
        repo.create_default_reader_code(owner, &competing),
    );
    let first = first.unwrap();
    let second = second.unwrap();
    assert_eq!(first, second, "simultaneous requests must return one code");
    assert_eq!(first.status, "active");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM referral_codes
         WHERE owner_user_id=$1 AND program_type='reader_spend'",
    )
    .bind(owner)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        repo.create_default_reader_code(owner, "r_different").await.unwrap(),
        first
    );
    sqlx::query("UPDATE referral_codes SET status='blocked' WHERE code=$1")
        .bind(&first.code)
        .execute(&pool)
        .await
        .unwrap();
    let blocked = repo
        .create_default_reader_code(owner, "r_attempt_to_bypass")
        .await
        .unwrap();
    assert_eq!(blocked.code, first.code);
    assert_eq!(blocked.status, "blocked");
    assert!(!repo.active_code(&blocked.code).await.unwrap());
}

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_pg_private_overview_exposes_only_owned_aggregates() {
    let pool = isolated_pool().await;
    let repo = PgReferrals::new(pool.clone());
    let inviter = seed_user(&pool, "overview-inviter").await;
    let another = seed_user(&pool, "overview-another").await;
    let invited = seed_user(&pool, "overview-invited").await;
    let code = repo
        .create_default_reader_code(inviter, &format!("r_owner_{inviter}"))
        .await
        .unwrap();
    let other_code = repo
        .create_default_reader_code(another, &format!("r_other_{another}"))
        .await
        .unwrap();
    enqueue(&pool, invited, None, Some(&code.code)).await;
    assert!(repo.finalize_one(invited).await.unwrap());

    let own = repo.reader_overview(inviter).await.unwrap();
    assert_eq!(own.default_code, Some(code));
    assert_eq!((own.active, own.under_review, own.blocked, own.expired), (1, 0, 0, 0));
    let other = repo.reader_overview(another).await.unwrap();
    assert_eq!(other.default_code, Some(other_code));
    assert_eq!((other.active, other.under_review, other.blocked, other.expired), (0, 0, 0, 0));
    let subject = repo.reader_overview(invited).await.unwrap();
    assert!(subject.default_code.is_none());
    assert_eq!(subject.active, 0);
}
