//! A-03/A-07: stale credential verification and simultaneous rotations.
use std::{
    num::NonZeroU32,
    time::{SystemTime, UNIX_EPOCH},
};
use tor_accounts::{
    domain::access_audit::{LoginAuditContext, LoginEventKind, UserAgentFamily},
    repositories::{credentials::PasswordRotation, PgAccounts},
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
            accounts.rotate_password_and_sessions(PasswordRotation {
                user_id: actor,
                old_session: &first,
                expected_password_hash: "$argon2id$old-hash",
                new_password_hash: "$argon2id$new-a",
                new_session: &a,
                ttl,
                audit: audit_a,
            }),
            accounts.rotate_password_and_sessions(PasswordRotation {
                user_id: actor,
                old_session: &second,
                expected_password_hash: "$argon2id$old-hash",
                new_password_hash: "$argon2id$new-b",
                new_session: &b,
                ttl,
                audit: audit_b,
            }),
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
    assert_eq!(
        event.0.as_deref(),
        Some(if won_a {
            "198.51.100.31"
        } else {
            "203.0.113.41"
        })
    );
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

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn a_07_admin_refund_step_up_binds_live_session_and_password_rotation() {
    use sha2::{Digest, Sha256};
    use tor_accounts::{
        repositories::step_up::StepUpIssueError,
        services::security::{hash_password, verify_password_for_step_up},
    };
    let url = std::env::var("TEST_DATABASE_URL").expect("TEST_DATABASE_URL");
    assert!(url.ends_with("/tor_ci"), "never use a real database");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(pool.clone());
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let first_hash = hash_password("ValidModeratorPassword2026").unwrap();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'A-owned test administrator',$2) RETURNING id",
    )
    .bind(format!("step_up_{unique}@example.test"))
    .bind(&first_hash)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active)
         VALUES($1,'administrator',TRUE)",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();

    let session_digest = session_token_digest(&format!("a07:step-up:{unique}"));
    accounts
        .insert_session(actor, &session_digest, NonZeroU32::new(3600).unwrap())
        .await
        .unwrap();
    let digest: [u8; 32] = Sha256::digest(b"exact refund approval intent").into();
    let verified = verify_password_for_step_up("ValidModeratorPassword2026", &first_hash).unwrap();
    let token = accounts
        .issue_administrator_refund_step_up(actor, &session_digest, &verified, 5, 1, &digest)
        .await
        .unwrap();

    let (stored_hash, stored_scope, resource, revision, payload_match, ttl_valid, used): (
        Vec<u8>,
        String,
        i64,
        i64,
        bool,
        bool,
        bool,
    ) = sqlx::query_as(
        "SELECT step_up_proof_hash,step_up_scope,step_up_resource_id,
                step_up_revision,(step_up_payload_digest=$2),
                (step_up_expires_at>step_up_issued_at AND
                 step_up_expires_at<=step_up_issued_at+INTERVAL '5 minutes'),
                step_up_used_at IS NOT NULL
         FROM sessions WHERE token_hash=$1",
    )
    .bind(session_digest.as_slice())
    .bind(digest.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored_hash, session_token_digest(&token).to_vec());
    assert_ne!(stored_hash.as_slice(), token.as_bytes());
    assert_eq!(stored_scope, "finance.refund.approve.v1");
    assert_eq!(
        (resource, revision, payload_match, ttl_valid, used),
        (5, 1, true, true, false)
    );
    assert_eq!(token.len(), 43);

    assert!(verify_password_for_step_up("wrong password", &first_hash).is_none());
    let rotated = hash_password("RotatedModeratorPassword2026").unwrap();
    sqlx::query("UPDATE users SET password_hash=$1 WHERE id=$2")
        .bind(rotated)
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        accounts
            .issue_administrator_refund_step_up(actor, &session_digest, &verified, 5, 1, &digest)
            .await,
        Err(StepUpIssueError::StepUpRequired)
    );
    sqlx::query("UPDATE sessions SET revoked_at=clock_timestamp() WHERE token_hash=$1")
        .bind(session_digest.as_slice())
        .execute(&pool)
        .await
        .unwrap();
    let newly_verified =
        verify_password_for_step_up("ValidModeratorPassword2026", &first_hash).unwrap();
    assert_eq!(
        accounts
            .issue_administrator_refund_step_up(
                actor,
                &session_digest,
                &newly_verified,
                5,
                1,
                &digest,
            )
            .await,
        Err(StepUpIssueError::StepUpRequired)
    );
    sqlx::query(
        "UPDATE account_roles SET is_active=FALSE
         WHERE user_id=$1 AND role='administrator'",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
}
