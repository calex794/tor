//! A-03/A-07: stale credential verification and simultaneous rotations.
use std::{num::NonZeroU32, time::{SystemTime, UNIX_EPOCH}};
use tor_accounts::{
    domain::access_audit::{LoginAuditContext, LoginEventKind, UserAgentFamily},
    repositories::PgAccounts,
    services::security::session_token_digest,
};

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn a_07_stale_login_and_concurrent_password_rotation_fail_closed() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1"));
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let ttl = NonZeroU32::new(3600).unwrap();
    let first = session_token_digest(&format!("first-{nonce}"));
    let second = session_token_digest(&format!("second-{nonce}"));
    let a = session_token_digest(&format!("rotate-a-{nonce}"));
    let b = session_token_digest(&format!("rotate-b-{nonce}"));
    let stale = session_token_digest(&format!("stale-{nonce}"));
    let actor = accounts.register_with_session(
        &format!("race-{nonce}@example.invalid"), "Race", "old-hash", &first, ttl,
    ).await.unwrap();
    accounts.insert_session(actor, &second, ttl).await.unwrap();
    let (ra, rb) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(
            accounts.rotate_password_and_sessions(actor, &first, "old-hash", "new-a", &a, ttl),
            accounts.rotate_password_and_sessions(actor, &second, "old-hash", "new-b", &b, ttl),
        )
    }).await.expect("account-first locks must not deadlock");
    assert_ne!(ra.unwrap(), rb.unwrap(), "exactly one credential CAS wins");
    let audit = LoginAuditContext {
        verified_ip: None, agent_family: UserAgentFamily::Unknown,
        event_kind: LoginEventKind::PasswordLogin,
    };
    assert!(!accounts.insert_verified_session(actor, "old-hash", &stale, ttl, audit).await.unwrap());
    assert_eq!(accounts.active_session_user(&first).await.unwrap(), None);
    assert_eq!(accounts.active_session_user(&second).await.unwrap(), None);
    assert_eq!(accounts.active_session_user(&stale).await.unwrap(), None);
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM sessions WHERE user_id=$1 AND revoked_at IS NULL",
    ).bind(actor).fetch_one(&pool).await.unwrap();
    assert_eq!(remaining, 1);
}
