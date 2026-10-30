//! A-03/A-07: stale credential verification and simultaneous rotations.
use std::{
    num::NonZeroU32,
    time::{SystemTime, UNIX_EPOCH},
};
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
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let ttl = NonZeroU32::new(3600).unwrap();
    let first = session_token_digest(&format!("first-{nonce}"));
    let second = session_token_digest(&format!("second-{nonce}"));
    let a = session_token_digest(&format!("rotate-a-{nonce}"));
    let b = session_token_digest(&format!("rotate-b-{nonce}"));
    let stale = session_token_digest(&format!("stale-{nonce}"));
    let actor = accounts
        .register_with_session(
            &format!("race-{nonce}@example.invalid"),
            "Race",
            "$argon2id$old-hash",
            &first,
            ttl,
        )
        .await
        .unwrap();
    accounts.insert_session(actor, &second, ttl).await.unwrap();
    let audit_a = LoginAuditContext {
        verified_ip: Some("198.51.100.31".parse().unwrap()),
        agent_family: UserAgentFamily::Firefox,
        event_kind: LoginEventKind::PasswordChange,
    };
    let audit_b = LoginAuditContext {
        verified_ip: Some("203.0.113.41".parse().unwrap()),
        agent_family: UserAgentFamily::Chrome,
        event_kind: LoginEventKind::PasswordChange,
    };
    let (ra, rb) = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        tokio::join!(
            accounts.rotate_password_and_sessions(
                actor,
                &first,
                "$argon2id$old-hash",
                "$argon2id$new-a",
                &a,
                ttl,
                audit_a
            ),
            accounts.rotate_password_and_sessions(
                actor,
                &second,
                "$argon2id$old-hash",
                "$argon2id$new-b",
                &b,
                ttl,
                audit_b
            ),
        )
    })
    .await
    .expect("account-first locks must not deadlock");
    let won_a = ra.unwrap();
    let won_b = rb.unwrap();
    assert_ne!(won_a, won_b, "exactly one credential CAS wins");
    let event: (Option<String>, String, String) = sqlx::query_as(
        "SELECT host(e.verified_ip), e.agent_family, e.event_kind
         FROM account_login_events e JOIN sessions s ON s.session_id=e.session_id
         WHERE s.user_id=$1 AND s.revoked_at IS NULL",
    )
    .bind(actor)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(event.0.as_deref(), Some(if won_a { "198.51.100.31" } else { "203.0.113.41" }));
    assert_eq!(event.1, if won_a { "firefox" } else { "chrome" });
    assert_eq!(event.2, "password_change");
    let audit = LoginAuditContext {
        verified_ip: None,
        agent_family: UserAgentFamily::Unknown,
        event_kind: LoginEventKind::PasswordLogin,
    };
    assert!(!accounts
        .insert_verified_session(actor, "$argon2id$old-hash", &stale, ttl, audit)
        .await
        .unwrap());
    assert_eq!(accounts.active_session_user(&first).await.unwrap(), None);
    assert_eq!(accounts.active_session_user(&second).await.unwrap(), None);
    assert_eq!(accounts.active_session_user(&stale).await.unwrap(), None);
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM sessions WHERE user_id=$1 AND revoked_at IS NULL")
            .bind(actor)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(remaining, 1);
    let change_events: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_login_events WHERE user_id=$1 AND event_kind='password_change'",
    )
    .bind(actor)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(change_events, 1);
}
