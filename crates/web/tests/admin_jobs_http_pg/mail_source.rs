//! Real PostgreSQL: source resolution only, never SMTP or enqueue.
use super::*;
use tor_contracts::{
    auth::{Actor, SessionActor, UserId},
    mail::{MailIntent, MailPurpose},
    paid_delivery::PositiveId,
};
use tor_mail::services::dispatch::{DispatchSource, SourceFailure};
use tor_support::{domain::Requester, repositories::postgres::PgSupport, services::SupportService};
use tor_web::mail_dispatch_source::SupportDispatchSource;

#[tokio::test]
#[ignore = "requires migrated disposable PostgreSQL tor_ci"]
async fn mail_04_support_reply_source_checks_owner_and_live_account() {
    let url = env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.starts_with("postgresql://tor_app:") && url.ends_with("/tor_ci"));
    let catalog = PgCatalog::connect(&url).await.unwrap();
    let pool = catalog.pool().clone();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut users = Vec::new();
    for index in 0..3 {
        let id: i64 = pg(
            &url,
            &format!(
                "INSERT INTO users(email_normalized,display_name,password_hash)
                 VALUES('mail-source-{nonce}-{index}@example.invalid','Mail source fixture','fixture')
                 RETURNING id"
            ),
        )
        .parse()
        .unwrap();
        users.push(id);
    }
    let requester = Requester::User {
        id: users[0],
        guest: None,
    };
    let service = SupportService::new(PgSupport::new(pool.clone()));
    let initial = service
        .send(&requester, "PRIVATE REQUEST", None)
        .await
        .unwrap();
    let chat = service
        .messages(&requester, None, None, 100)
        .await
        .unwrap()
        .chat;
    let actor = SessionActor {
        actor: Actor {
            user_id: UserId::new(users[2]).unwrap(),
        },
        author: false,
        moderator: false,
        administrator: true,
    };
    let reply = service
        .admin_send(actor, chat.id, "PRIVATE REPLY", None, "mail-source-fixture")
        .await
        .unwrap();
    let accounts = PgAccounts::from_pool(pool);
    let source = SupportDispatchSource::new(accounts.clone(), "https://example.invalid").unwrap();
    let intent = |event, recipient| MailIntent {
        purpose: MailPurpose::SupportReply,
        event_id: PositiveId::new(event).unwrap(),
        recipient: UserId::new(recipient).unwrap(),
    };

    let valid = intent(reply.id, users[0]);
    assert!(source.resolve(valid).await.is_ok());
    assert_eq!(
        source.resolve(intent(reply.id, users[1])).await.err(),
        Some(SourceFailure::Cancelled)
    );
    assert_eq!(
        source.resolve(intent(initial.id, users[0])).await.err(),
        Some(SourceFailure::Cancelled)
    );
    let other_purpose = MailIntent {
        purpose: MailPurpose::ProjectNews,
        ..valid
    };
    assert_eq!(
        source.resolve(other_purpose).await.err(),
        Some(SourceFailure::Unavailable)
    );
    // Re-resolving reads the current address, not a queued historical copy.
    pg(
        &url,
        &format!(
            "UPDATE users SET email_normalized='mail-source-new-{nonce}@example.invalid'
             WHERE id={}",
            users[0]
        ),
    );
    assert_eq!(
        accounts
            .mail_delivery_address(valid.recipient)
            .await
            .unwrap()
            .as_deref(),
        Some(format!("mail-source-new-{nonce}@example.invalid").as_str())
    );
    // Deleting the account removes the owned source, never sends to a stale address.
    pg(&url, &format!("DELETE FROM users WHERE id={}", users[0]));
    assert_eq!(
        source.resolve(valid).await.err(),
        Some(SourceFailure::Cancelled)
    );
    pg(
        &url,
        &format!("DELETE FROM users WHERE id IN ({},{})", users[1], users[2]),
    );
}
