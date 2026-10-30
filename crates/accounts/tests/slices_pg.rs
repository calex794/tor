//! Active line A acceptance against an isolated restricted PostgreSQL account.
use std::num::NonZeroU32;
use std::time::{SystemTime, UNIX_EPOCH};
use tor_accounts::{
    repositories::{authority::AuthorityError, profile::ProfileError, PgAccounts},
    services::{profile::ProfileUpdate, security::session_token_digest},
};
use tor_contracts::auth::{AccountRole, UserId};

async fn fixture(label: &str) -> (PgAccounts, i64, [u8; 32]) {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1"));
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(pool);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let digest = session_token_digest(&format!("{label}-{nonce}"));
    let user = accounts
        .register_with_session(
            &format!("{label}-{nonce}@example.invalid"),
            "Reader",
            "$argon2id$fixture",
            &digest,
            NonZeroU32::new(3600).unwrap(),
        )
        .await
        .unwrap();
    (accounts, user, digest)
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn a_06_profile_revision_ownership_handle_uniqueness_and_adult_flag() {
    let (accounts, alice, _) = fixture("profile-alice").await;
    let (_, bob, _) = fixture("profile-bob").await;
    let handle = format!("reader{}", alice);
    let original = accounts.profile(alice).await.unwrap().unwrap();
    assert!(!original.adult_enabled && original.revision == 0);
    let mut update = ProfileUpdate {
        display_name: Some("Updated reader".into()),
        handle: Some(handle.clone()),
        adult_enabled: Some(true),
        expected_revision: 0,
    };
    update.validate().unwrap();
    let (a, b) = tokio::join!(
        accounts.update_profile(alice, &update),
        accounts.update_profile(alice, &update)
    );
    assert!(matches!(
        (&a, &b),
        (Ok(1), Err(ProfileError::Stale)) | (Err(ProfileError::Stale), Ok(1))
    ));
    assert_eq!(
        accounts.profile(bob).await.unwrap().unwrap().display_name,
        "Reader"
    );
    assert!(matches!(
        accounts.update_profile(bob, &update).await,
        Err(ProfileError::Duplicate)
    ));
    assert_eq!(
        accounts.profile(bob).await.unwrap().unwrap().revision,
        0,
        "handle conflict rolls back name/adult writes too"
    );
    let saved = accounts.profile(alice).await.unwrap().unwrap();
    assert!(saved.adult_enabled);
    assert_eq!(saved.handle.as_deref(), Some(handle.as_str()));
    update.expected_revision = 1;
    update.handle = Some(String::new());
    update.adult_enabled = Some(false);
    accounts.update_profile(alice, &update).await.unwrap();
    let saved = accounts.profile(alice).await.unwrap().unwrap();
    assert!(saved.handle.is_none() && !saved.adult_enabled);
    let grants: i64 = sqlx::query_scalar("SELECT count(*) FROM user_level_grants WHERE user_id=$1")
        .bind(alice)
        .fetch_one(accounts.pool())
        .await
        .unwrap();
    assert_eq!(grants, 0, "adult consent cannot create a pro/paid grant");
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn a_07_reset_expiry_replay_parallel_consumption_and_credential_binding() {
    let (accounts, user, session) = fixture("reset").await;
    let token = session_token_digest(&format!("reset-proof-{user}"));
    assert!(accounts
        .issue_password_reset(user, "$argon2id$fixture", &token)
        .await
        .unwrap());
    assert_eq!(
        accounts
            .live_reset_password_hash(&token)
            .await
            .unwrap()
            .as_deref(),
        Some("$argon2id$fixture")
    );
    let (a, b) = tokio::join!(
        accounts.consume_password_reset(&token, "$argon2id$new-a"),
        accounts.consume_password_reset(&token, "$argon2id$new-b")
    );
    assert_ne!(a.unwrap(), b.unwrap());
    assert_eq!(accounts.active_session_user(&session).await.unwrap(), None);
    assert!(!accounts
        .consume_password_reset(&token, "$argon2id$replay")
        .await
        .unwrap());
    assert!(accounts
        .live_reset_password_hash(&token)
        .await
        .unwrap()
        .is_none());
    let hash = accounts
        .password_hash_for_user(user)
        .await
        .unwrap()
        .unwrap();
    let expired = session_token_digest(&format!("expired-{user}"));
    accounts
        .issue_password_reset(user, &hash, &expired)
        .await
        .unwrap();
    sqlx::query("UPDATE password_reset_tokens SET created_at=now()-INTERVAL '2 hours',expires_at=now()-INTERVAL '1 hour' WHERE token_hash=$1")
        .bind(expired.as_slice()).execute(accounts.pool()).await.unwrap();
    assert!(accounts
        .live_reset_password_hash(&expired)
        .await
        .unwrap()
        .is_none());
    assert!(!accounts
        .consume_password_reset(&expired, "$argon2id$expired")
        .await
        .unwrap());
    let stale = session_token_digest(&format!("stale-proof-{user}"));
    accounts
        .issue_password_reset(user, &hash, &stale)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET password_hash='$argon2id$rotated' WHERE id=$1")
        .bind(user)
        .execute(accounts.pool())
        .await
        .unwrap();
    assert!(accounts
        .live_reset_password_hash(&stale)
        .await
        .unwrap()
        .is_none());
    assert!(!accounts
        .consume_password_reset(&stale, "$argon2id$stale")
        .await
        .unwrap());
    assert!(!accounts
        .issue_password_reset(user, &hash, &token)
        .await
        .unwrap());
    assert!(!accounts
        .consume_password_reset(&[0; 32], "$argon2id$forged")
        .await
        .unwrap());
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn a_04_a_05_roles_are_live_audited_and_never_granted_by_identity_claims() {
    let (accounts, admin, admin_session) = fixture("admin").await;
    let (_, reader, reader_session) = fixture("role-reader").await;
    let target = UserId::new(reader).unwrap();
    assert!(matches!(
        accounts
            .change_account_role(&reader_session, target, AccountRole::Administrator, true)
            .await,
        Err(AuthorityError::Denied)
    ));
    // Bootstrap fixture only; the runtime exposes no self-admin grant.
    sqlx::query("INSERT INTO account_roles(user_id,role) VALUES($1,'administrator')")
        .bind(admin)
        .execute(accounts.pool())
        .await
        .unwrap();
    accounts
        .change_account_role(&admin_session, target, AccountRole::Author, true)
        .await
        .unwrap();
    let actor = accounts
        .session_actor(&reader_session)
        .await
        .unwrap()
        .unwrap();
    assert!(actor.author && !actor.administrator);
    assert_eq!(actor.actor.user_id, target);
    accounts
        .change_account_role(&admin_session, target, AccountRole::Author, false)
        .await
        .unwrap();
    assert!(
        !accounts
            .session_actor(&reader_session)
            .await
            .unwrap()
            .unwrap()
            .author
    );
    assert!(matches!(
        accounts
            .change_account_role(
                &admin_session,
                UserId::new(admin).unwrap(),
                AccountRole::Administrator,
                false
            )
            .await,
        Err(AuthorityError::LastAdministrator)
    ));
    let audit: Vec<String> =
        sqlx::query_scalar("SELECT value FROM account_role_events WHERE user_id=$1 ORDER BY id")
            .bind(reader)
            .fetch_all(accounts.pool())
            .await
            .unwrap();
    assert_eq!(audit, vec!["granted", "revoked"]);
    accounts.revoke_session(&admin_session).await.unwrap();
    assert!(matches!(
        accounts
            .change_account_role(&admin_session, target, AccountRole::Author, true)
            .await,
        Err(AuthorityError::Denied)
    ));
    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
        .bind(reader)
        .execute(accounts.pool())
        .await
        .unwrap();
    assert!(accounts
        .session_actor(&reader_session)
        .await
        .unwrap()
        .is_none());
}
