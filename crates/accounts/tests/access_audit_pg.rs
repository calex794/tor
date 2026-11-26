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
        row.1.as_deref() == Some("198.51.100.12") && row.2 == "chrome" && row.3 == "registration"
    }));
    assert!(rows.iter().any(|row| {
        row.1.as_deref() == Some("2001:db8::99") && row.2 == "firefox" && row.3 == "password_login"
    }));
    assert!(rows
        .iter()
        .any(|row| { row.1.is_none() && row.2 == "unknown" && row.3 == "password_login" }));

    let replay = other_instance
        .insert_audited_session(user_id, &first, ttl, registration)
        .await;
    assert!(replay.is_err(), "reused digest cannot add a second session");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM account_login_events WHERE user_id=$1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .expect("row count");
    assert_eq!(
        count, 3,
        "failed insert must not produce an orphan audit event"
    );

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
    let remaining: i64 =
        sqlx::query_scalar("SELECT count(*) FROM account_login_events WHERE user_id=$1")
            .bind(user_id)
            .fetch_one(&pool)
            .await
            .expect("audit retention");
    assert_eq!(remaining, 3);
}

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci; not production"]
async fn auth_04_admin_session_revocation_rechecks_authority_and_audits_pg() {
    use tor_accounts::{
        repositories::admin_sessions::AdminSessionRevokeError,
        services::security::{hash_password, verify_password_for_step_up},
    };

    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"));
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let ttl = NonZeroU32::new(3600).unwrap();
    let admin = session_token_digest(&format!("admin-revoke-{nonce}"));
    let victim = session_token_digest(&format!("target-revoke-{nonce}"));
    let second = session_token_digest(&format!("second-revoke-{nonce}"));
    let foreign = session_token_digest(&format!("foreign-revoke-{nonce}"));
    let password = "AdminSessionPassword2026";
    let hash = hash_password(password).unwrap();
    let admin_id = accounts
        .register_with_session(
            &format!("admin-revoke-{nonce}@example.invalid"),
            "Admin",
            &hash,
            &admin,
            ttl,
        )
        .await
        .unwrap();
    let target = accounts
        .register_with_session(
            &format!("target-revoke-{nonce}@example.invalid"),
            "Target",
            "$argon2id$fixture",
            &victim,
            ttl,
        )
        .await
        .unwrap();
    accounts.insert_session(target, &second, ttl).await.unwrap();
    let foreign_id = accounts
        .register_with_session(
            &format!("foreign-revoke-{nonce}@example.invalid"),
            "Foreign",
            "$argon2id$fixture",
            &foreign,
            ttl,
        )
        .await
        .unwrap();
    let proof = verify_password_for_step_up(password, &hash).unwrap();
    assert_eq!(
        accounts
            .revoke_administrator_target_sessions(&admin, target, None, &proof)
            .await,
        Err(AdminSessionRevokeError::Denied)
    );
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'administrator',TRUE)",
    )
    .bind(admin_id)
    .execute(&pool)
    .await
    .unwrap();
    let selected: String =
        sqlx::query_scalar("SELECT session_id::text FROM sessions WHERE token_hash=$1")
            .bind(victim.as_slice())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        accounts
            .revoke_administrator_target_sessions(&admin, admin_id, None, &proof)
            .await,
        Err(AdminSessionRevokeError::InvalidTarget)
    );
    // A session ID belonging to the target cannot revoke a different user.
    assert_eq!(
        accounts
            .revoke_administrator_target_sessions(&admin, foreign_id, Some(&selected), &proof)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        accounts
            .revoke_administrator_target_sessions(&admin, target, Some(&selected), &proof)
            .await
            .unwrap(),
        1
    );
    assert_eq!(accounts.active_session_user(&victim).await.unwrap(), None);
    assert_eq!(
        accounts.active_session_user(&second).await.unwrap(),
        Some(target)
    );
    assert_eq!(
        accounts
            .revoke_administrator_target_sessions(&admin, target, None, &proof)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        accounts
            .revoke_administrator_target_sessions(&admin, target, None, &proof)
            .await
            .unwrap(),
        0
    );
    let facts: Vec<(Option<String>, i64)> = sqlx::query_as(
        "SELECT selected_session_id::text,revoked_count
         FROM account_admin_session_revocation_events
         WHERE administrator_user_id=$1 AND target_user_id=$2 ORDER BY id",
    )
    .bind(admin_id)
    .bind(target)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(facts, vec![(Some(selected), 1), (None, 1)]);
    assert_eq!(accounts.active_session_user(&second).await.unwrap(), None);
    assert_eq!(
        accounts.active_session_user(&foreign).await.unwrap(),
        Some(foreign_id)
    );
    assert_eq!(
        accounts.active_session_user(&admin).await.unwrap(),
        Some(admin_id)
    );
    assert!(sqlx::query(
        "UPDATE account_admin_session_revocation_events SET revoked_count=2
         WHERE administrator_user_id=$1"
    )
    .bind(admin_id)
    .execute(&pool)
    .await
    .is_err());
    let new_target = session_token_digest(&format!("new-target-{nonce}"));
    accounts
        .insert_session(target, &new_target, ttl)
        .await
        .unwrap();
    let history = accounts.security_history(&new_target).await.unwrap();
    assert_eq!(
        history
            .iter()
            .filter(|event| event.event_kind == "admin_revoke" && event.revoked_count == Some(1))
            .count(),
        2
    );
    assert!(accounts
        .security_history(&victim)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(accounts.renew_session_on_activity(&victim).await.unwrap(), None);
    assert_eq!(accounts.renew_session_on_activity(&second).await.unwrap(), None);
    sqlx::query(
        "UPDATE account_roles SET is_active=FALSE
         WHERE user_id=$1 AND role='administrator'",
    )
    .bind(admin_id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        accounts
            .revoke_administrator_target_sessions(&admin, target, None, &proof)
            .await,
        Err(AdminSessionRevokeError::Denied)
    );
    assert_eq!(
        accounts.active_session_user(&new_target).await.unwrap(),
        Some(target)
    );
}
