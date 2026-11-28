use super::*;
use tor_contracts::{auth::UserId, mail::SignupMailSource};

fn source(ip: Option<[u8; 32]>) -> SignupMailSource {
    SignupMailSource {
        recipient_key: [17; 32],
        registration_ip_key: ip,
        key_binding: [29; 32],
    }
}

async fn signup(
    accounts: &PgAccounts,
    email: &str,
    code: Option<&str>,
    evidence: Option<SignupMailSource>,
) -> Result<i64, AccountWriteError> {
    accounts
        .register_with_audited_session_referral_mail(
            email,
            "Mail signup fixture",
            "$argon2id$test",
            &session_token_digest(&generate_session_token()),
            NonZeroU32::new(3600).unwrap(),
            RegistrationContext {
                referral: RegistrationReferral {
                    guest_hash: None,
                    manual_code: code,
                },
                audit: LoginAuditContext {
                    verified_ip: None,
                    agent_family: UserAgentFamily::Unknown,
                    event_kind: LoginEventKind::Registration,
                },
            },
            evidence,
        )
        .await
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL 18 tor_ci"]
async fn mail_01_f2b_signup_evidence_is_atomic_and_session_independent() {
    let db = pool().await;
    let accounts = PgAccounts::from_pool(db.clone());
    let evidence = source(Some([37; 32]));
    let email = unique_email("mail-source");
    let id = signup(&accounts, &email, None, Some(evidence))
        .await
        .unwrap();
    let user = UserId::new(id).unwrap();
    let committed = accounts
        .registration_mail_source(user)
        .await
        .unwrap()
        .unwrap();
    assert!(committed.source == evidence);
    assert_eq!(committed.user_id, user);
    assert!(matches!(
        signup(&accounts, &email, None, Some(source(None))).await,
        Err(AccountWriteError::Duplicate)
    ));
    sqlx::query("DELETE FROM sessions WHERE user_id=$1")
        .bind(id)
        .execute(&db)
        .await
        .unwrap();
    let restarted = PgAccounts::from_pool(db.clone());
    let again = restarted
        .registration_mail_source(user)
        .await
        .unwrap()
        .unwrap();
    assert!(again.source == evidence);
    let rejected = unique_email("mail-source-rejected");
    assert!(matches!(
        signup(
            &accounts,
            &rejected,
            Some("unknown_mail_fixture_code"),
            Some(evidence)
        )
        .await,
        Err(AccountWriteError::InvalidReferral)
    ));
    assert!(accounts.find_user(&rejected).await.unwrap().is_none());

    let legacy = signup(&accounts, &unique_email("mail-off"), None, None)
        .await
        .unwrap();
    let unknown = signup(
        &accounts,
        &unique_email("mail-unknown"),
        None,
        Some(source(None)),
    )
    .await
    .unwrap();
    assert!(accounts
        .registration_mail_source(UserId::new(legacy).unwrap())
        .await
        .unwrap()
        .is_none());
    let captured = accounts
        .registration_mail_source(UserId::new(unknown).unwrap())
        .await
        .unwrap()
        .unwrap();
    assert!(captured.source.registration_ip_key.is_none());

    sqlx::query(
        "UPDATE account_registration_mail_sources SET created_at=clock_timestamp()-interval '8 days' WHERE user_id=$1",
    )
    .bind(id)
    .execute(&db)
    .await
    .unwrap();
    assert!(accounts
        .registration_mail_source(user)
        .await
        .unwrap()
        .is_none());
    accounts.prune_security_data().await.unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_registration_mail_sources WHERE user_id=$1",
    )
    .bind(id)
    .fetch_one(&db)
    .await
    .unwrap();
    assert_eq!(count, 0);
    for fixture in [id, legacy, unknown] {
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(fixture)
            .execute(&db)
            .await
            .unwrap();
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL 18 tor_ci"]
async fn mail_05_email_ownership_is_one_use_and_bound_to_live_mailbox() {
    let db = pool().await;
    let accounts = PgAccounts::from_pool(db.clone());
    let email = unique_email("mail-ownership");
    let id = signup(&accounts, &email, None, None).await.unwrap();
    let other_id = signup(&accounts, &unique_email("mail-other"), None, None)
        .await
        .unwrap();
    let user = UserId::new(id).unwrap();
    let other = UserId::new(other_id).unwrap();
    let candidate = accounts.mailbox_candidate(user).await.unwrap().unwrap();
    assert_eq!(candidate.transport_address(), email);
    assert!(accounts
        .verified_mail_delivery_address(user)
        .await
        .unwrap()
        .is_none());

    let first = session_token_digest(&generate_session_token());
    let second = session_token_digest(&generate_session_token());
    assert!(accounts
        .issue_email_ownership_proof(&candidate, &first)
        .await
        .unwrap());
    assert!(!accounts
        .issue_email_ownership_proof(&candidate, &second)
        .await
        .unwrap());
    assert!(!accounts
        .consume_email_ownership_proof(other, &first)
        .await
        .unwrap());
    assert!(!accounts
        .consume_email_ownership_proof(user, &second)
        .await
        .unwrap());
    sqlx::query(
        "UPDATE account_email_ownership
         SET issued_at=statement_timestamp()-interval '31 minutes',
             expires_at=statement_timestamp()-interval '1 minute'
         WHERE user_id=$1",
    )
    .bind(id)
    .execute(&db)
    .await
    .unwrap();
    assert!(!accounts
        .consume_email_ownership_proof(user, &first)
        .await
        .unwrap());
    assert!(accounts
        .issue_email_ownership_proof(&candidate, &second)
        .await
        .unwrap());
    let (a, b) = tokio::join!(
        accounts.consume_email_ownership_proof(user, &second),
        accounts.consume_email_ownership_proof(user, &second)
    );
    assert_ne!(a.unwrap(), b.unwrap());
    assert!(!accounts
        .consume_email_ownership_proof(user, &second)
        .await
        .unwrap());
    assert_eq!(
        accounts
            .verified_mail_delivery_address(user)
            .await
            .unwrap()
            .as_deref(),
        Some(email.as_str())
    );
    assert!(!accounts
        .issue_email_ownership_proof(&candidate, &first)
        .await
        .unwrap());

    // A changed address invalidates proof and outstanding source. Changing
    // it back does not resurrect the previous verified ownership.
    sqlx::query("UPDATE users SET email_normalized=$2 WHERE id=$1")
        .bind(id)
        .bind(unique_email("mail-changed"))
        .execute(&db)
        .await
        .unwrap();
    assert!(!accounts
        .issue_email_ownership_proof(&candidate, &first)
        .await
        .unwrap());
    sqlx::query("UPDATE users SET email_normalized=$2 WHERE id=$1")
        .bind(id)
        .bind(&email)
        .execute(&db)
        .await
        .unwrap();
    assert!(accounts
        .verified_mail_delivery_address(user)
        .await
        .unwrap()
        .is_none());
    assert!(!accounts
        .issue_email_ownership_proof(&candidate, &first)
        .await
        .unwrap());
    let fresh = accounts.mailbox_candidate(user).await.unwrap().unwrap();
    let third = session_token_digest(&generate_session_token());
    assert!(accounts
        .issue_email_ownership_proof(&fresh, &third)
        .await
        .unwrap());
    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
        .bind(id)
        .execute(&db)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET account_state='active' WHERE id=$1")
        .bind(id)
        .execute(&db)
        .await
        .unwrap();
    assert!(!accounts
        .consume_email_ownership_proof(user, &third)
        .await
        .unwrap());
    for id in [id, other_id] {
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(id)
            .execute(&db)
            .await
            .unwrap();
    }
}

#[path = "registration_delivery.rs"]
mod registration_delivery;
