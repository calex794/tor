//! Initial signup source and handoff: disposable PG, no SMTP or queue worker.
use super::*;
use tor_contracts::{
    mail::{MailIntent, MailPurpose},
    paid_delivery::PositiveId,
};

fn intent(id: i64) -> MailIntent {
    MailIntent {
        purpose: MailPurpose::RegistrationConfirmation,
        event_id: PositiveId::new(id).unwrap(),
        recipient: UserId::new(id).unwrap(),
    }
}

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL 18 tor_ci"]
async fn mail_05_registration_handoff_is_exact_bounded_and_invalidated() {
    let db = pool().await;
    let accounts = PgAccounts::from_pool(db.clone());
    let email = unique_email("registration-delivery");
    let captured = source(Some([17; 32]));
    let id = signup(&accounts, &email, None, Some(captured))
        .await
        .unwrap();
    let other = signup(&accounts, &unique_email("registration-no-source"), None, None)
        .await
        .unwrap();
    let request = intent(id);
    let wrong = MailIntent {
        event_id: PositiveId::new(other).unwrap(),
        ..request
    };
    assert!(accounts.resolve_registration_mail(wrong).await.unwrap().is_none());
    assert!(!accounts.mark_registration_mail_enqueued(wrong).await.unwrap());
    assert!(accounts
        .resolve_registration_mail(intent(other))
        .await
        .unwrap()
        .is_none());
    let (candidate, evidence) = accounts
        .resolve_registration_mail(request)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(candidate.transport_address(), email);
    assert!(evidence.source == captured);
    assert_eq!(evidence.user_id, request.recipient);

    let pending = accounts.pending_registration_mail_intents(1000).await.unwrap();
    assert!(pending.len() <= 100);
    assert!(pending.contains(&request));
    assert!(!pending.contains(&intent(other)));
    assert!(accounts.pending_registration_mail_intents(0).await.unwrap().is_empty());
    assert!(accounts.mark_registration_mail_enqueued(request).await.unwrap());
    assert!(!accounts.mark_registration_mail_enqueued(request).await.unwrap());
    let restarted = PgAccounts::from_pool(db.clone());
    assert!(!restarted.pending_registration_mail_intents(100).await.unwrap().contains(&request));
    assert!(restarted.resolve_registration_mail(request).await.unwrap().is_some());

    sqlx::query("UPDATE account_registration_mail_sources SET created_at=clock_timestamp()-interval '25 hours' WHERE user_id=$1")
        .bind(id).execute(&db).await.unwrap();
    assert!(restarted.resolve_registration_mail(request).await.unwrap().is_none());
    sqlx::query("UPDATE account_registration_mail_sources SET created_at=clock_timestamp() WHERE user_id=$1")
        .bind(id).execute(&db).await.unwrap();

    let proof = session_token_digest(&generate_session_token());
    assert!(accounts.issue_email_ownership_proof(&candidate, &proof).await.unwrap());
    assert!(accounts.consume_email_ownership_proof(request.recipient, &proof).await.unwrap());
    assert!(accounts.resolve_registration_mail(request).await.unwrap().is_none());

    sqlx::query("UPDATE users SET email_normalized=$2 WHERE id=$1")
        .bind(id).bind(unique_email("registration-changed")).execute(&db).await.unwrap();
    sqlx::query("UPDATE users SET email_normalized=$2 WHERE id=$1")
        .bind(id).bind(&email).execute(&db).await.unwrap();
    assert!(accounts.resolve_registration_mail(request).await.unwrap().is_none());
    assert!(accounts.registration_mail_source(request.recipient).await.unwrap().is_none());
    assert!(!accounts.issue_email_ownership_proof(&candidate, &proof).await.unwrap());

    for id in [id, other] {
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(id)
            .execute(&db)
            .await
            .unwrap();
    }
}
