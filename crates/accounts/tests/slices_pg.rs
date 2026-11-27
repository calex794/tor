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
async fn a_05_authority_pool_acquisition_is_bounded_and_recovers() {
    use std::time::{Duration, Instant};
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1"));
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .acquire_timeout(Duration::from_secs(5))
        .connect(&url)
        .await
        .unwrap();
    let accounts = PgAccounts::from_pool(pool.clone());
    let held = pool.acquire().await.unwrap();
    let started = Instant::now();
    let result = accounts.session_actor(&[0; 32]).await;
    assert!(matches!(result, Err(sqlx::Error::PoolTimedOut)));
    assert!(started.elapsed() < Duration::from_millis(600));
    drop(held);
    assert!(accounts.session_actor(&[0; 32]).await.unwrap().is_none());
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
async fn auth_05_security_history_is_owner_scoped_bounded_and_durable() {
    use tor_accounts::domain::access_audit::{LoginAuditContext, LoginEventKind, UserAgentFamily};

    let (accounts, user, current) = fixture("security-history").await;
    let (_, foreign_user, foreign) = fixture("security-history-foreign").await;
    let ttl = NonZeroU32::new(3600).unwrap();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let login = session_token_digest(&format!("history-login-{nonce}"));
    let logout = session_token_digest(&format!("history-logout-{nonce}"));
    let reset = session_token_digest(&format!("history-reset-{nonce}"));
    let after_reset = session_token_digest(&format!("history-after-reset-{nonce}"));
    let audit = LoginAuditContext {
        verified_ip: Some("198.51.100.5".parse().unwrap()),
        agent_family: UserAgentFamily::Firefox,
        event_kind: LoginEventKind::PasswordLogin,
    };
    accounts
        .insert_verified_session(user, "$argon2id$fixture", &login, ttl, audit)
        .await
        .unwrap();
    accounts.insert_session(user, &logout, ttl).await.unwrap();
    let selected = accounts
        .list_active_sessions(user, &login)
        .await
        .unwrap()
        .into_iter()
        .find(|session| session.current)
        .unwrap()
        .id;
    assert_eq!(
        accounts
            .revoke_owned_session(user, &current, &selected, "wrong-hash")
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        accounts
            .revoke_owned_session(user, &current, &selected, "$argon2id$fixture")
            .await
            .unwrap(),
        Some(false)
    );
    assert_eq!(
        accounts
            .revoke_owned_session(user, &current, &selected, "$argon2id$fixture")
            .await
            .unwrap(),
        None
    );
    assert!(accounts.revoke_session(&logout).await.unwrap());
    assert!(!accounts.revoke_session(&logout).await.unwrap());
    assert!(accounts.security_history(&login).await.unwrap().is_empty());
    let history = accounts.security_history(&current).await.unwrap();
    assert_eq!(history.len(), 3);
    assert!(history.iter().any(|event| {
        event.event_kind == "password_login"
            && event.browser_family.as_deref() == Some("firefox")
            && event.revoked_count.is_none()
    }));
    for kind in ["owner_revoke", "session_ended"] {
        assert_eq!(
            history
                .iter()
                .filter(|event| event.event_kind == kind && event.revoked_count == Some(1))
                .count(),
            1
        );
    }
    assert!(accounts
        .security_history(&foreign)
        .await
        .unwrap()
        .is_empty());
    assert!(accounts
        .security_history(&[0; 32])
        .await
        .unwrap()
        .is_empty());
    assert!(accounts
        .issue_password_reset(user, "$argon2id$fixture", &reset)
        .await
        .unwrap());
    assert!(accounts
        .consume_password_reset(&reset, "$argon2id$reset-hash")
        .await
        .unwrap());
    assert!(!accounts
        .consume_password_reset(&reset, "$argon2id$replayed")
        .await
        .unwrap());
    assert!(accounts
        .security_history(&current)
        .await
        .unwrap()
        .is_empty());
    assert!(accounts
        .insert_verified_session(user, "$argon2id$reset-hash", &after_reset, ttl, audit)
        .await
        .unwrap());
    let history = accounts.security_history(&after_reset).await.unwrap();
    assert_eq!(history.len(), 5);
    assert_eq!(
        history
            .iter()
            .filter(|event| event.event_kind == "password_reset" && event.revoked_count == Some(1))
            .count(),
        1
    );
    assert_eq!(
        accounts.active_session_user(&foreign).await.unwrap(),
        Some(foreign_user)
    );

    // Expired historical records are excluded before cleanup; recent records
    // cannot be rewritten by the runtime role.
    sqlx::query(
        "INSERT INTO account_security_events(user_id,event_kind,revoked_count,created_at)
         VALUES($1,'password_reset',0,now()-INTERVAL '91 days')",
    )
    .bind(user)
    .execute(accounts.pool())
    .await
    .unwrap();
    assert_eq!(
        accounts.security_history(&after_reset).await.unwrap().len(),
        5
    );
    assert!(
        sqlx::query("UPDATE account_security_events SET revoked_count=9 WHERE user_id=$1")
            .bind(user)
            .execute(accounts.pool())
            .await
            .is_err()
    );
    accounts.prune_security_data().await.unwrap();
    let old: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_security_events
         WHERE user_id=$1 AND created_at<now()-INTERVAL '90 days'",
    )
    .bind(user)
    .fetch_one(accounts.pool())
    .await
    .unwrap();
    assert_eq!(old, 0);
    sqlx::query(
        "INSERT INTO account_security_events(user_id,event_kind,revoked_count)
         SELECT $1,'password_reset',0 FROM generate_series(1,55)",
    )
    .bind(user)
    .execute(accounts.pool())
    .await
    .unwrap();
    assert_eq!(
        accounts.security_history(&after_reset).await.unwrap().len(),
        50
    );
    assert!(accounts
        .security_history(&foreign)
        .await
        .unwrap()
        .is_empty());
}

/// Named signup must not strand accounts or claim a handle after rollback.
#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL"]
async fn auth_03_named_signup_is_atomic_and_uses_existing_handle_uniqueness() {
    use tor_accounts::{
        domain::access_audit::{LoginAuditContext, LoginEventKind, UserAgentFamily},
        repositories::postgres::{AccountWriteError, RegistrationContext, RegistrationReferral},
        services::validation::named_registration,
    };
    use tor_contracts::mail::SignupMailSource;

    let (accounts, _, existing_digest) = fixture("named-signup").await;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let handle = format!("u{nonce:x}");
    let email_a = format!("named-a-{nonce}@example.invalid");
    let email_b = format!("named-b-{nonce}@example.invalid");
    let email_c = format!("named-c-{nonce}@example.invalid");
    let password = "NamedFixturePassword2026";
    let a = named_registration(&email_a, &handle, password, password).unwrap();
    let b = named_registration(&email_b, &handle.to_uppercase(), password, password).unwrap();
    let digest_a = session_token_digest(&email_a);
    let digest_b = session_token_digest(&email_b);
    let ttl = NonZeroU32::new(3600).unwrap();
    let context = || RegistrationContext {
        referral: RegistrationReferral {
            guest_hash: None,
            manual_code: None,
        },
        audit: LoginAuditContext {
            verified_ip: None,
            agent_family: UserAgentFamily::Unknown,
            event_kind: LoginEventKind::Registration,
        },
    };
    let mail = SignupMailSource {
        recipient_key: [17; 32],
        registration_ip_key: None,
        key_binding: [23; 32],
    };
    let (ra, rb) = tokio::join!(
        accounts.register_named(&a, "$argon2id$fixture", &digest_a, ttl, context(), Some(mail)),
        accounts.register_named(&b, "$argon2id$fixture", &digest_b, ttl, context(), Some(mail)),
    );
    let (winner, won_email, lost_email, won_digest, lost_digest) = match (ra, rb) {
        (Ok(id), Err(AccountWriteError::Duplicate)) => {
            (id, &email_a, &email_b, &digest_a, &digest_b)
        }
        (Err(AccountWriteError::Duplicate), Ok(id)) => {
            (id, &email_b, &email_a, &digest_b, &digest_a)
        }
        _ => panic!("exactly one normalized handle can commit"),
    };
    assert!(accounts.find_user(lost_email).await.unwrap().is_none());
    assert_eq!(accounts.active_session_user(lost_digest).await.unwrap(), None);
    assert_eq!(
        accounts.active_session_user(won_digest).await.unwrap(),
        Some(winner)
    );
    let profile = accounts.profile(winner).await.unwrap().unwrap();
    assert_eq!(profile.handle.as_deref(), Some(handle.as_str()));
    assert_eq!(profile.display_name, handle);
    assert_eq!(profile.revision, 1);
    let evidence = accounts
        .registration_mail_source(UserId::new(winner).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(evidence.source == mail);

    // A conflicting session digest fails after the handle insert. The entire
    // signup must roll back, leaving both email and handle reusable.
    let other_handle = format!("v{nonce:x}");
    let c = named_registration(&email_c, &other_handle, password, password).unwrap();
    assert_eq!(
        accounts
            .register_named(
                &c,
                "$argon2id$fixture",
                &existing_digest,
                ttl,
                context(),
                Some(mail),
            )
            .await,
        Err(AccountWriteError::Duplicate)
    );
    assert!(accounts.find_user(&email_c).await.unwrap().is_none());
    let saved: i64 =
        sqlx::query_scalar("SELECT count(*) FROM account_settings WHERE handle=$1")
            .bind(&other_handle)
            .fetch_one(accounts.pool())
            .await
            .unwrap();
    assert_eq!(saved, 0);
    // Duplicate email also cannot reserve an otherwise free handle.
    let duplicate = named_registration(won_email, &other_handle, password, password).unwrap();
    let digest_c = session_token_digest(&email_c);
    assert_eq!(
        accounts
            .register_named(
                &duplicate,
                "$argon2id$fixture",
                &digest_c,
                ttl,
                context(),
                None,
            )
            .await,
        Err(AccountWriteError::Duplicate)
    );
    assert!(accounts
        .register_named(&c, "$argon2id$fixture", &digest_c, ttl, context(), Some(mail))
        .await
        .is_ok());
}
