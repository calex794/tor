//! ACCESS-AUDIT-01/02: PostgreSQL persistence and session identity.
use std::{
    net::IpAddr,
    num::NonZeroU32,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    domain::access_audit::{LoginAuditContext, LoginEventKind, UserAgentFamily},
    repositories::PgAccounts,
    services::security::session_token_digest,
};

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci; not production"]
async fn a03_account_login_events_follow_exact_sessions_and_revoke() {
    let url = std::env::var("TEST_DATABASE_URL").expect("CI database URL");
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let pool = sqlx::PgPool::connect(&url).await.expect("PostgreSQL 18");
    let accounts = PgAccounts::from_pool(pool.clone());
    accounts.probe().await.expect("restricted tor_app schema");

    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("wall clock")
        .as_nanos();
    let email = format!("pg-audit-{nonce}@example.invalid");
    let ttl = NonZeroU32::new(3600).expect("positive TTL");
    let first = session_token_digest(&format!("pg-audit-first-{nonce}"));
    let second = session_token_digest(&format!("pg-audit-second-{nonce}"));
    let third = session_token_digest(&format!("pg-audit-unknown-{nonce}"));
    let registration = LoginAuditContext {
        verified_ip: Some("198.51.100.12".parse::<IpAddr>().unwrap()),
        agent_family: UserAgentFamily::Chrome,
        event_kind: LoginEventKind::Registration,
    };
    let user_id = accounts
        .register_with_audited_session(
            &email,
            "Audit Reader",
            "$argon2id$fixture",
            &first,
            ttl,
            registration,
        )
        .await
        .expect("atomic registration and audit");

    let login = LoginAuditContext {
        verified_ip: Some("2001:db8::99".parse::<IpAddr>().unwrap()),
        agent_family: UserAgentFamily::Firefox,
        event_kind: LoginEventKind::PasswordLogin,
    };
    // A separate adapter instance sees the same durable state.
    let other_instance = PgAccounts::from_pool(pool.clone());
    other_instance
        .insert_audited_session(user_id, &second, ttl, login)
        .await
        .expect("second session and event");
    other_instance
        .insert_audited_session(
            user_id,
            &third,
            ttl,
            LoginAuditContext {
                verified_ip: None,
                agent_family: UserAgentFamily::Unknown,
                event_kind: LoginEventKind::PasswordLogin,
            },
        )
        .await
        .expect("unknown IP stays null");

    let rows: Vec<(String, Option<String>, String, String)> = sqlx::query_as(
        "SELECT e.session_id::text, host(e.verified_ip), e.agent_family, e.event_kind \
         FROM account_login_events e \
         JOIN sessions s ON s.session_id=e.session_id AND s.user_id=e.user_id \
         WHERE e.user_id=$1 ORDER BY e.created_at, e.session_id",
    )
    .bind(user_id)
    .fetch_all(&pool)
    .await
    .expect("private login audit rows");
    assert_eq!(rows.len(), 3);
    assert_ne!(rows[0].0, rows[1].0);
    assert!(rows.iter().any(|row| {
        row.1.as_deref() == Some("198.51.100.12")
            && row.2 == "chrome"
            && row.3 == "registration"
    }));
    assert!(rows.iter().any(|row| {
        row.1.as_deref() == Some("2001:db8::99")
            && row.2 == "firefox"
            && row.3 == "password_login"
    }));
    assert!(rows.iter().any(|row| {
        row.1.is_none() && row.2 == "unknown" && row.3 == "password_login"
    }));

    let replay = other_instance
        .insert_audited_session(user_id, &first, ttl, registration)
        .await;
    assert!(replay.is_err(), "reused digest cannot add a second session");
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_login_events WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .expect("row count");
    assert_eq!(count, 3, "failed insert must not produce an orphan audit event");

    assert_eq!(
        other_instance
            .active_session_user(&first)
            .await
            .expect("existing session"),
        Some(user_id)
    );
    assert!(accounts.revoke_session(&first).await.expect("revoke"));
    assert_eq!(
        other_instance
            .active_session_user(&first)
            .await
            .expect("revoked session"),
        None
    );
    // Revoking a session must not erase the security event.
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_login_events WHERE user_id=$1",
    )
    .bind(user_id)
    .fetch_one(&pool)
    .await
    .expect("audit retention");
    assert_eq!(remaining, 3);
}
