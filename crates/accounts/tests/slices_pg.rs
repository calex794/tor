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

/// Source display-name keys ignore case; ambiguous legacy keys must never
/// silently resolve to the first account or reveal any different account.
#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn e_08_public_display_name_resolution_fails_closed_on_case_collision() {
    let (accounts, alice, _) = fixture("e08-profile-alice").await;
    let (_, bob, _) = fixture("e08-profile-bob").await;
    let display_name = format!("Mail Reader {alice}");
    let colliding_name = format!("mAiL rEaDeR {alice}");
    accounts
        .update_profile(
            alice,
            &ProfileUpdate {
                display_name: Some(display_name.clone()),
                handle: None,
                adult_enabled: None,
                expected_revision: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        accounts
            .public_profile_by_display_name(&colliding_name)
            .await
            .unwrap(),
        Some((alice, display_name.clone())),
    );
    assert_eq!(
        accounts
            .public_profile_by_display_name(&format!("  {colliding_name}  "))
            .await
            .unwrap(),
        Some((alice, display_name.clone())),
    );
    assert_eq!(
        accounts.public_profile_by_display_name("  ").await.unwrap(),
        None
    );
    accounts
        .update_profile(
            bob,
            &ProfileUpdate {
                display_name: Some(colliding_name.clone()),
                handle: None,
                adult_enabled: None,
                expected_revision: 0,
            },
        )
        .await
        .unwrap();
    for key in [&display_name, &colliding_name] {
        assert_eq!(
            accounts.public_profile_by_display_name(key).await.unwrap(),
            None
        );
    }
    assert_eq!(
        accounts.profile(alice).await.unwrap().unwrap().display_name,
        display_name
    );
    assert_eq!(
        accounts.profile(bob).await.unwrap().unwrap().display_name,
        colliding_name
    );
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

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn a_05_moderator_is_live_revocable_and_never_an_administrator() {
    let (accounts, admin, admin_session) = fixture("a05-mod-admin").await;
    let (_, moderator, moderator_session) = fixture("a05-moderator").await;
    let (_, outsider, outsider_session) = fixture("a05-mod-outsider").await;
    let target = UserId::new(moderator).unwrap();
    sqlx::query("INSERT INTO account_roles(user_id,role) VALUES($1,'administrator')")
        .bind(admin)
        .execute(accounts.pool())
        .await
        .unwrap();

    // A normal authenticated account cannot assign the moderator role.
    assert!(matches!(
        accounts
            .change_account_role(&outsider_session, target, AccountRole::Moderator, true)
            .await,
        Err(AuthorityError::Denied)
    ));
    let before = accounts
        .session_actor(&moderator_session)
        .await
        .unwrap()
        .unwrap();
    assert!(!before.moderator && !before.administrator && !before.author);

    accounts
        .change_account_role(&admin_session, target, AccountRole::Moderator, true)
        .await
        .unwrap();
    let live = accounts
        .session_actor(&moderator_session)
        .await
        .unwrap()
        .unwrap();
    assert!(live.moderator);
    assert!(!live.administrator && !live.author);
    assert_eq!(live.actor.user_id, target);

    // The new role is not a site administrator and cannot grant other roles.
    assert!(matches!(
        accounts
            .change_account_role(
                &moderator_session,
                UserId::new(outsider).unwrap(),
                AccountRole::Administrator,
                true
            )
            .await,
        Err(AuthorityError::Denied)
    ));
    accounts
        .change_account_role(&admin_session, target, AccountRole::Moderator, false)
        .await
        .unwrap();
    let revoked = accounts
        .session_actor(&moderator_session)
        .await
        .unwrap()
        .unwrap();
    assert!(!revoked.moderator && !revoked.administrator);
    let values: Vec<String> = sqlx::query_scalar(
        "SELECT value FROM account_role_events
         WHERE user_id=$1 AND role='moderator' ORDER BY id",
    )
    .bind(moderator)
    .fetch_all(accounts.pool())
    .await
    .unwrap();
    assert_eq!(values, vec!["granted", "revoked"]);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn a_05_role_grants_reject_inactive_targets_without_audit_or_dormant_privilege() {
    let (accounts, admin, admin_session) = fixture("a05-grant-admin").await;
    let (_, recipient, _) = fixture("a05-grant-recipient").await;
    let target = UserId::new(recipient).unwrap();

    // Only the isolated test fixture bootstraps the first administrator.
    sqlx::query("INSERT INTO account_roles(user_id,role) VALUES($1,'administrator')")
        .bind(admin)
        .execute(accounts.pool())
        .await
        .unwrap();
    for state in ["suspended", "closed"] {
        sqlx::query("UPDATE users SET account_state=$1 WHERE id=$2")
            .bind(state)
            .bind(recipient)
            .execute(accounts.pool())
            .await
            .unwrap();
        for role in [
            AccountRole::Administrator,
            AccountRole::Moderator,
            AccountRole::Author,
        ] {
            assert!(matches!(
                accounts
                    .change_account_role(&admin_session, target, role, true)
                    .await,
                Err(AuthorityError::Denied)
            ));
        }
    }
    let stored: i64 = sqlx::query_scalar("SELECT count(*) FROM account_roles WHERE user_id=$1")
        .bind(recipient)
        .fetch_one(accounts.pool())
        .await
        .unwrap();
    assert_eq!(stored, 0, "denied grant must not leave a dormant privilege");
    let denied_events: i64 =
        sqlx::query_scalar("SELECT count(*) FROM account_role_events WHERE user_id=$1")
            .bind(recipient)
            .fetch_one(accounts.pool())
            .await
            .unwrap();
    assert_eq!(
        denied_events, 0,
        "denied grant must not be audited as success"
    );

    sqlx::query("UPDATE users SET account_state='active' WHERE id=$1")
        .bind(recipient)
        .execute(accounts.pool())
        .await
        .unwrap();
    let new_session = session_token_digest(&format!("a05-active-again-{recipient}"));
    accounts
        .insert_session(recipient, &new_session, NonZeroU32::new(3600).unwrap())
        .await
        .unwrap();
    assert!(
        !accounts
            .session_actor(&new_session)
            .await
            .unwrap()
            .unwrap()
            .administrator
    );
    accounts
        .change_account_role(&admin_session, target, AccountRole::Administrator, true)
        .await
        .unwrap();
    assert!(
        accounts
            .session_actor(&new_session)
            .await
            .unwrap()
            .unwrap()
            .administrator
    );
    accounts
        .change_account_role(&admin_session, target, AccountRole::Administrator, false)
        .await
        .unwrap();
    assert!(
        !accounts
            .session_actor(&new_session)
            .await
            .unwrap()
            .unwrap()
            .administrator
    );
    let events: Vec<String> =
        sqlx::query_scalar("SELECT value FROM account_role_events WHERE user_id=$1 ORDER BY id")
            .bind(recipient)
            .fetch_all(accounts.pool())
            .await
            .unwrap();
    assert_eq!(events, vec!["granted", "revoked"]);
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn a_07_telegram_pairing_requires_live_session_and_owner_confirmation() {
    let (accounts, alice, session) = fixture("tg-link-alice").await;
    let (_, bob, other_session) = fixture("tg-link-bob").await;
    let nonce = session_token_digest(&format!("tg-bind-{alice}"));
    assert!(!accounts
        .begin_telegram_link(alice, &other_session, "$argon2id$fixture", &nonce)
        .await
        .unwrap());
    assert!(!accounts
        .begin_telegram_link(alice, &session, "$argon2id$wrong", &nonce)
        .await
        .unwrap());
    assert!(accounts
        .begin_telegram_link(alice, &session, "$argon2id$fixture", &nonce)
        .await
        .unwrap());
    let tg_id = 7_000_000 + alice;
    assert!(!accounts
        .claim_telegram_link(&nonce, tg_id, tg_id + 1)
        .await
        .unwrap());
    assert!(!accounts
        .claim_telegram_link(&[0_u8; 32], tg_id, tg_id)
        .await
        .unwrap());
    assert!(accounts
        .claim_telegram_link(&nonce, tg_id, tg_id)
        .await
        .unwrap());
    let state = accounts.telegram_link_state(alice, &session).await.unwrap();
    assert_eq!(state.candidate_user_id, Some(tg_id));
    let codes: [[u8; 32]; 8] =
        std::array::from_fn(|i| session_token_digest(&format!("tg-backup-{alice}-{i}")));
    assert!(accounts
        .finish_telegram_link(alice, &other_session, &codes)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        accounts
            .finish_telegram_link(alice, &session, &codes)
            .await
            .unwrap(),
        Some(tg_id)
    );
    assert_eq!(
        accounts
            .telegram_link_state(alice, &session)
            .await
            .unwrap()
            .linked_user_id,
        Some(tg_id)
    );
    assert!(accounts
        .finish_telegram_link(alice, &session, &codes)
        .await
        .unwrap()
        .is_none());
    assert!(!accounts
        .claim_telegram_link(&nonce, tg_id, tg_id)
        .await
        .unwrap());
    let other_nonce = session_token_digest(&format!("tg-bind-bob-{bob}"));
    assert!(accounts
        .begin_telegram_link(bob, &other_session, "$argon2id$fixture", &other_nonce)
        .await
        .unwrap());
    assert!(!accounts
        .claim_telegram_link(&other_nonce, tg_id, tg_id)
        .await
        .unwrap());
    assert!(!accounts.telegram_update_seen(tg_id).await.unwrap());
    assert!(accounts.mark_telegram_update(tg_id).await.unwrap());
    assert!(accounts.telegram_update_seen(tg_id).await.unwrap());
    assert!(!accounts.mark_telegram_update(tg_id).await.unwrap());
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn a_07_telegram_recovery_backup_one_time_role_and_channel_guards() {
    let (accounts, user, session) = fixture("tg-reset").await;
    let tg_id = 8_000_000 + user;
    let nonce = session_token_digest(&format!("tg-bind-reset-{user}"));
    assert!(accounts
        .begin_telegram_link(user, &session, "$argon2id$fixture", &nonce)
        .await
        .unwrap());
    assert!(accounts
        .claim_telegram_link(&nonce, tg_id, tg_id)
        .await
        .unwrap());
    let codes: [[u8; 32]; 8] =
        std::array::from_fn(|i| session_token_digest(&format!("tg-code-{user}-{i}")));
    assert_eq!(
        accounts
            .finish_telegram_link(user, &session, &codes)
            .await
            .unwrap(),
        Some(tg_id)
    );
    let email: String = sqlx::query_scalar("SELECT email_normalized FROM users WHERE id=$1")
        .bind(user)
        .fetch_one(accounts.pool())
        .await
        .unwrap();
    let browser = session_token_digest(&format!("tg-browser-{user}"));
    accounts
        .begin_browser_telegram_recovery(Some(&email), &browser, "Abc_12")
        .await
        .unwrap();
    let view = accounts
        .browser_telegram_recovery(&browser)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(view.match_code, "Abc_12");
    assert_eq!(view.status, "pending");
    let pending = accounts
        .pending_telegram_recovery_notifications()
        .await
        .unwrap();
    let request_id = pending
        .iter()
        .find(|x| x.private_chat_id == tg_id)
        .unwrap()
        .id;
    assert!(!accounts
        .decide_telegram_recovery(request_id, tg_id + 1, tg_id, true)
        .await
        .unwrap());
    assert!(!accounts
        .decide_telegram_recovery(request_id, tg_id, tg_id + 1, true)
        .await
        .unwrap());
    assert!(accounts
        .decide_telegram_recovery(request_id, tg_id, tg_id, true)
        .await
        .unwrap());
    assert!(!accounts
        .decide_telegram_recovery(request_id, tg_id, tg_id, true)
        .await
        .unwrap());
    let proof = session_token_digest(&format!("tg-reset-proof-{user}"));
    assert!(!accounts
        .exchange_browser_telegram_approval(&[0_u8; 32], &proof)
        .await
        .unwrap());
    assert!(accounts
        .exchange_browser_telegram_approval(&browser, &proof)
        .await
        .unwrap());
    assert!(!accounts
        .exchange_browser_telegram_approval(&browser, &[1_u8; 32])
        .await
        .unwrap());
    assert_eq!(
        accounts
            .live_reset_password_hash_allowed(&proof, false, false)
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        accounts
            .live_reset_password_hash_allowed(&proof, false, true)
            .await
            .unwrap()
            .as_deref(),
        Some("$argon2id$fixture")
    );
    sqlx::query("INSERT INTO account_roles(user_id,role) VALUES($1,'author')")
        .bind(user)
        .execute(accounts.pool())
        .await
        .unwrap();
    assert!(accounts
        .live_reset_password_hash_allowed(&proof, false, true)
        .await
        .unwrap()
        .is_none());
    assert!(!accounts
        .consume_password_reset_allowed(&proof, "$argon2id$denied", false, true)
        .await
        .unwrap());
    sqlx::query("DELETE FROM account_roles WHERE user_id=$1 AND role='author'")
        .bind(user)
        .execute(accounts.pool())
        .await
        .unwrap();
    assert!(accounts
        .consume_password_reset_allowed(&proof, "$argon2id$changed", false, true)
        .await
        .unwrap());
    assert_eq!(accounts.active_session_user(&session).await.unwrap(), None);
    assert!(!accounts
        .consume_password_reset_allowed(&proof, "$argon2id$repeat", false, true)
        .await
        .unwrap());
    let backup_token = session_token_digest(&format!("backup-reset-{user}"));
    let (a, b) = tokio::join!(
        accounts.consume_backup_code(&email, &codes[0], &backup_token),
        accounts.consume_backup_code(&email, &codes[0], &backup_token)
    );
    assert_ne!(a.unwrap(), b.unwrap(), "backup code cannot be replayed");
    assert!(accounts
        .live_reset_password_hash_allowed(&backup_token, false, true)
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        accounts
            .live_reset_password_hash_allowed(&backup_token, false, false)
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn a_07_telegram_unlink_requires_live_owner_password_and_revokes_recovery_proofs() {
    let (accounts, user, session) = fixture("tg-unlink").await;
    let (_, stranger, stranger_session) = fixture("tg-unlink-other").await;
    let nonce = session_token_digest(&format!("tg-unlink-link-{user}"));
    let telegram_id = 10_000_000 + user;
    assert!(accounts
        .begin_telegram_link(user, &session, "$argon2id$fixture", &nonce)
        .await
        .unwrap());
    assert!(accounts
        .claim_telegram_link(&nonce, telegram_id, telegram_id)
        .await
        .unwrap());
    let codes: [[u8; 32]; 8] =
        std::array::from_fn(|i| session_token_digest(&format!("unlink-backup-{user}-{i}")));
    assert_eq!(
        accounts
            .finish_telegram_link(user, &session, &codes)
            .await
            .unwrap(),
        Some(telegram_id)
    );
    let email: String = sqlx::query_scalar("SELECT email_normalized FROM users WHERE id=$1")
        .bind(user)
        .fetch_one(accounts.pool())
        .await
        .unwrap();
    let proof = session_token_digest(&format!("telegram-preunlink-{user}"));
    let browser = session_token_digest(&format!("telegram-unlink-browser-{user}"));
    accounts
        .begin_browser_telegram_recovery(Some(&email), &browser, "Unl_42")
        .await
        .unwrap();
    let pending = accounts
        .pending_telegram_recovery_notifications()
        .await
        .unwrap();
    let request_id = pending
        .iter()
        .find(|x| x.private_chat_id == telegram_id)
        .unwrap()
        .id;
    assert!(accounts
        .decide_telegram_recovery(request_id, telegram_id, telegram_id, true)
        .await
        .unwrap());
    assert!(accounts
        .exchange_browser_telegram_approval(&browser, &proof)
        .await
        .unwrap());
    assert!(!accounts
        .unlink_telegram(user, &stranger_session, "$argon2id$fixture")
        .await
        .unwrap());
    assert!(!accounts
        .unlink_telegram(user, &session, "$argon2id$wrong")
        .await
        .unwrap());
    assert!(accounts
        .unlink_telegram(user, &session, "$argon2id$fixture")
        .await
        .unwrap());
    assert!(!accounts
        .unlink_telegram(user, &session, "$argon2id$fixture")
        .await
        .unwrap());
    assert_eq!(
        accounts
            .telegram_link_state(user, &session)
            .await
            .unwrap()
            .linked_user_id,
        None
    );
    let unlinked_browser = session_token_digest(&format!("unlinked-browser-{user}"));
    accounts
        .begin_browser_telegram_recovery(Some(&email), &unlinked_browser, "Unl_43")
        .await
        .unwrap();
    assert!(!accounts
        .pending_telegram_recovery_notifications()
        .await
        .unwrap()
        .iter()
        .any(|x| x.private_chat_id == telegram_id));
    assert_eq!(
        accounts
            .live_reset_password_hash_allowed(&proof, false, true)
            .await
            .unwrap(),
        None
    );
    let backup = session_token_digest(&format!("after-unlink-{user}"));
    assert!(!accounts
        .consume_backup_code(&email, &codes[0], &backup)
        .await
        .unwrap());
    let audit: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM account_telegram_binding_events
         WHERE user_id=$1 ORDER BY id",
    )
    .bind(user)
    .fetch_all(accounts.pool())
    .await
    .unwrap();
    assert_eq!(audit, vec!["linked", "unlinked"]);
    assert_ne!(user, stranger);
}
