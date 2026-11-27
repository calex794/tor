use super::*;
use tor_contracts::mail::{MailPurpose, PasswordResetMailSource};

fn source(ip: Option<[u8; 32]>) -> PasswordResetMailSource {
    PasswordResetMailSource {
        recipient_key: [17; 32],
        request_ip_key: ip,
        key_binding: [29; 32],
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable tor_ci PostgreSQL"]
async fn a_07_reset_mail_source_bounded_handoff_and_revocation() {
    let (accounts, user, _) = fixture("reset-mail-source").await;
    let db = accounts.pool();
    let email: String = sqlx::query_scalar("SELECT email_normalized FROM users WHERE id=$1")
        .bind(user)
        .fetch_one(db)
        .await
        .unwrap();
    let current = accounts.find_user(&email).await.unwrap().unwrap();
    let proof = session_token_digest("existing-reset-proof");
    assert!(accounts
        .issue_password_reset(user, &current.password_hash, &proof)
        .await
        .unwrap());

    let evidence = source(Some([37; 32]));
    assert!(accounts
        .capture_password_reset_mail_request("absent-reset@example.invalid", evidence)
        .await
        .unwrap()
        .is_none());
    let (a, b) = tokio::join!(
        accounts.capture_password_reset_mail_request(&email, evidence),
        accounts.capture_password_reset_mail_request(&email, source(None)),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a.is_some(), b.is_some());
    let expected = if a.is_some() { evidence } else { source(None) };
    let request = a.or(b).unwrap();
    assert_eq!(request.recipient, UserId::new(user).unwrap());
    assert_eq!(request.purpose, MailPurpose::PasswordReset);
    assert!(accounts
        .live_reset_password_hash(&proof)
        .await
        .unwrap()
        .is_some());
    let restarted = PgAccounts::from_pool(db.clone());
    let (resolved, address) = restarted
        .resolve_password_reset_mail_request(request)
        .await
        .unwrap()
        .unwrap();
    assert!(resolved.source == expected);
    assert_eq!(resolved.intent, request);
    assert_eq!(address, email);
    let mut wrong = request;
    wrong.purpose = MailPurpose::RegistrationConfirmation;
    assert!(accounts
        .resolve_password_reset_mail_request(wrong)
        .await
        .unwrap()
        .is_none());
    assert!(!accounts
        .mark_password_reset_mail_enqueued(wrong)
        .await
        .unwrap());
    let (_, foreign, _) = fixture("reset-mail-foreign").await;
    wrong = request;
    wrong.recipient = UserId::new(foreign).unwrap();
    assert!(accounts
        .resolve_password_reset_mail_request(wrong)
        .await
        .unwrap()
        .is_none());
    assert!(!accounts
        .mark_password_reset_mail_enqueued(wrong)
        .await
        .unwrap());
    assert!(restarted
        .pending_password_reset_mail_intents(100)
        .await
        .unwrap()
        .contains(&request));
    assert!(accounts
        .pending_password_reset_mail_intents(0)
        .await
        .unwrap()
        .is_empty());
    assert!(accounts
        .mark_password_reset_mail_enqueued(request)
        .await
        .unwrap());
    assert!(!accounts
        .mark_password_reset_mail_enqueued(request)
        .await
        .unwrap());
    assert!(!accounts
        .pending_password_reset_mail_intents(100)
        .await
        .unwrap()
        .contains(&request));

    sqlx::query(
        "UPDATE account_password_reset_mail_requests \
         SET created_at=clock_timestamp()-interval '121 seconds' WHERE user_id=$1",
    )
    .bind(user)
    .execute(db)
    .await
    .unwrap();
    let replacement = accounts
        .capture_password_reset_mail_request(&email, evidence)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(request.event_id, replacement.event_id);
    assert!(!accounts
        .mark_password_reset_mail_enqueued(request)
        .await
        .unwrap());
    assert!(accounts
        .resolve_password_reset_mail_request(request)
        .await
        .unwrap()
        .is_none());
    assert!(accounts
        .live_reset_password_hash(&proof)
        .await
        .unwrap()
        .is_some());

    sqlx::query(
        "UPDATE account_password_reset_mail_requests \
         SET created_at=clock_timestamp()-interval '11 minutes' WHERE user_id=$1",
    )
    .bind(user)
    .execute(db)
    .await
    .unwrap();
    assert!(accounts
        .resolve_password_reset_mail_request(replacement)
        .await
        .unwrap()
        .is_none());
    assert!(!accounts
        .mark_password_reset_mail_enqueued(replacement)
        .await
        .unwrap());
    accounts.prune_security_data().await.unwrap();
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM account_password_reset_mail_requests WHERE user_id=$1",
    )
    .bind(user)
    .fetch_one(db)
    .await
    .unwrap();
    assert_eq!(remaining, 0);

    let latest = accounts
        .capture_password_reset_mail_request(&email, evidence)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE users SET password_hash='$argon2id$new-fixture' WHERE id=$1")
        .bind(user)
        .execute(db)
        .await
        .unwrap();
    assert!(accounts
        .resolve_password_reset_mail_request(latest)
        .await
        .unwrap()
        .is_none());
    let latest = accounts
        .capture_password_reset_mail_request(&email, evidence)
        .await
        .unwrap()
        .unwrap();
    let new_email = format!("changed-reset-{user}@example.invalid");
    sqlx::query("UPDATE users SET email_normalized=$2 WHERE id=$1")
        .bind(user)
        .bind(&new_email)
        .execute(db)
        .await
        .unwrap();
    assert!(accounts
        .resolve_password_reset_mail_request(latest)
        .await
        .unwrap()
        .is_none());
    let latest = accounts
        .capture_password_reset_mail_request(&new_email, evidence)
        .await
        .unwrap()
        .unwrap();
    sqlx::query("UPDATE users SET account_state='suspended' WHERE id=$1")
        .bind(user)
        .execute(db)
        .await
        .unwrap();
    assert!(accounts
        .capture_password_reset_mail_request(&new_email, evidence)
        .await
        .unwrap()
        .is_none());
    sqlx::query("UPDATE users SET account_state='active' WHERE id=$1")
        .bind(user)
        .execute(db)
        .await
        .unwrap();
    assert!(accounts
        .resolve_password_reset_mail_request(latest)
        .await
        .unwrap()
        .is_none());
}
