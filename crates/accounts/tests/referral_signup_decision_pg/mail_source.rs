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
