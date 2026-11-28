//! A-03/A-09: private shared login budget and transactional reset.
use super::*;
use tor_accounts::services::{
    login_identity::{login_identity, submitted_attempt_key},
    profile::ProfileUpdate,
};

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn auth_03_shared_account_attempts_and_atomic_reset() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1"));
    let accounts = PgAccounts::from_pool(sqlx::PgPool::connect(&url).await.unwrap());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let email = login_identity(&format!("budget-{nonce}@example.invalid"), "fixture").unwrap();
    let handle = login_identity(&format!("b{nonce:x}"), "fixture").unwrap();
    let stale = login_identity(&format!("s{nonce:x}"), "fixture").unwrap();
    let ttl = NonZeroU32::new(3600).unwrap();
    let original = session_token_digest(&format!("budget-original-{nonce}"));
    let fresh = session_token_digest(&format!("budget-fresh-{nonce}"));
    let hash = "$argon2id$budget-test-fixture";
    let user = accounts
        .register_with_session(email.as_str(), "Budget", hash, &original, ttl)
        .await
        .unwrap();
    let mut claim = ProfileUpdate {
        display_name: None,
        handle: Some(handle.as_str().to_owned()),
        adult_enabled: None,
        expected_revision: 0,
    };
    claim.validate().unwrap();
    assert_eq!(accounts.update_profile(user, &claim).await.unwrap(), 1);
    for alias in [&email, &handle] {
        assert_eq!(
            accounts.find_user_by_login(alias).await.unwrap().unwrap().id,
            user
        );
    }
    assert!(accounts.find_user_by_login(&stale).await.unwrap().is_none());

    let submitted = submitted_attempt_key("login", email.as_str());
    let other_submitted = submitted_attempt_key("login", handle.as_str());
    assert!(accounts
        .permit_auth_attempt(&other_submitted, 1, 600)
        .await
        .unwrap());
    assert!(accounts
        .permit_auth_attempt(&submitted, 1, 600)
        .await
        .unwrap());

    for i in 0..7 {
        let alias = if i % 2 == 0 { &email } else { &handle };
        assert!(accounts
            .permit_login_account_attempt(Some(user), alias)
            .await
            .unwrap());
    }
    let (a, b) = tokio::join!(
        accounts.permit_login_account_attempt(Some(user), &email),
        accounts.permit_login_account_attempt(Some(user), &handle),
    );
    assert_ne!(a.unwrap(), b.unwrap());
    assert!(!accounts
        .permit_login_account_attempt(Some(user), &email)
        .await
        .unwrap());
    assert!(accounts
        .permit_login_account_attempt(None, &stale)
        .await
        .unwrap());
    let audit = LoginAuditContext {
        verified_ip: None,
        agent_family: UserAgentFamily::Unknown,
        event_kind: LoginEventKind::PasswordLogin,
    };
    // This alternate handle was never claimed. A stale alias cannot clear the budget.
    assert!(!accounts
        .insert_verified_session_for_login(user, hash, &fresh, ttl, audit, &stale)
        .await
        .unwrap());
    assert!(!accounts
        .permit_login_account_attempt(Some(user), &email)
        .await
        .unwrap());
    // Duplicate token fails the session insertion; counter deletes must roll back.
    assert!(accounts
        .insert_verified_session_for_login(user, hash, &original, ttl, audit, &email)
        .await
        .is_err());
    assert!(!accounts
        .permit_auth_attempt(&submitted, 1, 600)
        .await
        .unwrap());
    assert!(!accounts
        .permit_login_account_attempt(Some(user), &handle)
        .await
        .unwrap());

    // A caller with an already verified proof may issue a session and reset
    // both counters; HTTP must obtain account admission before verifying.
    assert!(accounts
        .insert_verified_session_for_login(user, hash, &fresh, ttl, audit, &email)
        .await
        .unwrap());
    assert_eq!(
        accounts.active_session_user(&fresh).await.unwrap(),
        Some(user)
    );
    assert!(accounts
        .permit_auth_attempt(&submitted, 1, 600)
        .await
        .unwrap());
    assert!(accounts
        .permit_login_account_attempt(Some(user), &handle)
        .await
        .unwrap());
    // The successful email login did not clear the public handle bucket.
    assert!(!accounts
        .permit_auth_attempt(&other_submitted, 1, 600)
        .await
        .unwrap());
}
