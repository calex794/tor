//! A-02/F-09: the account transaction freezes its referral decision.
//! Test-only direct SQL is confined to the owning PostgreSQL integration test.
use sqlx::PgPool;
use std::{
    num::NonZeroU32,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    domain::access_audit::{LoginAuditContext, LoginEventKind, UserAgentFamily},
    repositories::{
        postgres::{AccountWriteError, RegistrationContext, RegistrationReferral},
        PgAccounts,
    },
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
         VALUES($1,'F09 fixture','$argon2id$test') RETURNING id",
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
    let user_id = PgAccounts::from_pool(pool.clone())
        .register_with_audited_session_referral(
            &unique_email("invited"),
            "F09 invited",
            "$argon2id$test",
            &digest,
            NonZeroU32::new(3600).unwrap(),
            RegistrationContext {
                referral: RegistrationReferral {
                    guest_hash: guest,
                    manual_code: manual,
                },
                audit: LoginAuditContext {
                    verified_ip: None,
                    agent_family: UserAgentFamily::Unknown,
                    event_kind: LoginEventKind::Registration,
                },
            },
        )
        .await
        .expect("account, session and referral decision commit atomically");
    let audited: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_login_events e
         JOIN sessions s ON s.session_id=e.session_id AND s.user_id=e.user_id
         WHERE e.user_id=$1 AND e.event_kind='registration'",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .expect("persisted audited registration session");
    assert_eq!(
        audited, 1,
        "every referral signup must audit its first session"
    );
    user_id
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
    let status: String = sqlx::query_scalar("SELECT status FROM referral_captures WHERE id=$1")
        .bind(capture_id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(status, "consumed", "signup consumes its capture atomically");
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
    let status: String = sqlx::query_scalar("SELECT status FROM referral_captures WHERE id=$1")
        .bind(capture_id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(
        status, "consumed",
        "manual override must spend the old guest capture"
    );

    sqlx::query("UPDATE referral_codes SET status='disabled' WHERE code=$1")
        .bind(&second_code)
        .execute(&db)
        .await
        .unwrap();
    // A fresh guest capture exercises the legacy invalid-manual fallback.
    // The already-consumed hash must not be reusable for this second signup.
    let fresh_hash = session_token_digest(&generate_session_token());
    let fresh_capture_id = seed_capture(&db, &fresh_hash, &first_code).await;
    let fallback = register(&db, Some(&fresh_hash), Some(&second_code)).await;
    let decided = snapshot(&db, fallback).await;
    assert_eq!(decided.2, Some(first_owner));
    assert_eq!(decided.4, Some(fresh_capture_id));
    assert_eq!(decided.5, Some("guest_capture".into()));
}

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_pg_signup_cannot_claim_later_click() {
    let db = pool().await;
    let (_, code) = seed_code(&db, "reader_spend").await;
    let hash = session_token_digest(&generate_session_token());
    let subject = register(&db, Some(&hash), None).await;
    assert_eq!(
        snapshot(&db, subject).await,
        (true, None, None, None, None, None)
    );
    seed_capture(&db, &hash, &code).await;
    assert_eq!(
        snapshot(&db, subject).await,
        (true, None, None, None, None, None)
    );
}

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_pg_one_guest_capture_cannot_register_two_invitees() {
    let db = pool().await;
    let (owner, code) = seed_code(&db, "reader_spend").await;
    let hash = session_token_digest(&generate_session_token());
    let capture = seed_capture(&db, &hash, &code).await;

    let first = register(&db, Some(&hash), None).await;
    assert_eq!(snapshot(&db, first).await.2, Some(owner));
    assert_eq!(snapshot(&db, first).await.4, Some(capture));

    let second = register(&db, Some(&hash), None).await;
    assert_eq!(
        snapshot(&db, second).await,
        (true, None, None, None, None, None)
    );

    let repo = tor_referrals::repositories::PgReferrals::new(db.clone());
    assert_eq!(
        repo.capture(&hash, &code, "direct_ref_link", "/r/replay")
            .await
            .unwrap(),
        tor_referrals::repositories::postgres::CaptureResult::Existing,
        "replayed guest cookie must not create a new active capture"
    );
    let (count, status): (i64, String) = sqlx::query_as(
        "SELECT count(*) OVER(), status
         FROM referral_captures WHERE guest_hash=$1",
    )
    .bind(hash.as_slice())
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(count, 1);
    assert_eq!(status, "consumed");
    sqlx::query("UPDATE referral_captures SET status='blocked' WHERE id=$1")
        .bind(capture)
        .execute(&db)
        .await
        .unwrap();
    assert_eq!(
        repo.capture(&hash, &code, "direct_ref_link", "/r/after-block")
            .await
            .unwrap(),
        tor_referrals::repositories::postgres::CaptureResult::Existing,
        "moderating a spent capture must not reopen the bearer for replay"
    );
    assert!(repo.finalize_one(first).await.unwrap());
    assert!(!repo.finalize_one(second).await.unwrap());
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM referral_attributions
         WHERE source_capture_id=$1",
    )
    .bind(capture)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_pg_concurrent_signup_serializes_guest_claim() {
    let db = pool().await;
    let (owner, code) = seed_code(&db, "reader_spend").await;
    let hash = session_token_digest(&generate_session_token());
    let capture = seed_capture(&db, &hash, &code).await;

    let (first, second) = tokio::join!(
        register(&db, Some(&hash), None),
        register(&db, Some(&hash), None),
    );
    let snapshots = [snapshot(&db, first).await, snapshot(&db, second).await];
    let attributed: Vec<_> = snapshots
        .iter()
        .filter(|snapshot| snapshot.2 == Some(owner))
        .collect();
    assert_eq!(attributed.len(), 1, "only one signup may claim the guest");
    assert_eq!(attributed[0].4, Some(capture));
    assert_eq!(
        snapshots
            .iter()
            .filter(|snapshot| snapshot.1.is_none())
            .count(),
        1,
        "losing signup has an immutable no-referral decision"
    );
    let status: String = sqlx::query_scalar("SELECT status FROM referral_captures WHERE id=$1")
        .bind(capture)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(status, "consumed");
}

#[tokio::test]
#[ignore = "requires migrated PostgreSQL 18 tor_ci"]
async fn f_09_pg_disabled_manual_code_rolls_back_audited_signup_and_guest_claim() {
    let db = pool().await;
    let (_, manual) = seed_code(&db, "reader_spend").await;
    let (_, guest) = seed_code(&db, "reader_spend").await;
    let guest_hash = session_token_digest(&generate_session_token());
    let capture = seed_capture(&db, &guest_hash, &guest).await;
    sqlx::query("UPDATE referral_codes SET status='disabled' WHERE code=$1")
        .bind(&manual)
        .execute(&db)
        .await
        .unwrap();

    let email = unique_email("disabled-manual");
    let digest = session_token_digest(&generate_session_token());
    let result = PgAccounts::from_pool(db.clone())
        .register_with_audited_session_referral(
            &email,
            "Rejected signup",
            "$argon2id$test",
            &digest,
            NonZeroU32::new(3600).unwrap(),
            RegistrationContext {
                referral: RegistrationReferral {
                    guest_hash: Some(&guest_hash),
                    manual_code: Some(&manual),
                },
                audit: LoginAuditContext {
                    verified_ip: None,
                    agent_family: UserAgentFamily::Unknown,
                    event_kind: LoginEventKind::Registration,
                },
            },
        )
        .await;
    assert!(matches!(result, Err(AccountWriteError::InvalidReferral)));
    let users: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE email_normalized=$1")
        .bind(&email)
        .fetch_one(&db)
        .await
        .unwrap();
    let sessions: i64 = sqlx::query_scalar("SELECT count(*) FROM sessions WHERE token_hash=$1")
        .bind(digest.as_slice())
        .fetch_one(&db)
        .await
        .unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM referral_captures WHERE id=$1")
        .bind(capture)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(
        (users, sessions),
        (0, 0),
        "failed signup must leave no account or audited session"
    );
    assert_eq!(
        status, "active",
        "rejected manual code must not consume the guest capture"
    );
}
