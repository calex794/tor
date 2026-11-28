//! Disposable PostgreSQL acceptance: exact support source, without transport.
use super::*;
use tor_contracts::{
    mail::{MailIntent, MailPurpose},
    paid_delivery::PositiveId,
};

fn intent(event: i64, user: i64) -> MailIntent {
    MailIntent {
        purpose: MailPurpose::SupportReply,
        event_id: PositiveId::new(event).unwrap(),
        recipient: UserId::new(user).unwrap(),
    }
}

#[tokio::test]
#[ignore = "isolated TEST_DATABASE_URL ending /tor_ci"]
async fn mail_05_support_source_requires_owned_audited_admin_reply() {
    let (pool, users) = fixture().await;
    let repo = PgSupport::new(pool.clone());
    let service = SupportService::new(repo.clone());
    let requester = Requester::User {
        id: users[0],
        guest: None,
    };
    let initial = service
        .send(&requester, "PRIVATE REQUEST", None)
        .await
        .unwrap();
    let chat = service
        .messages(&requester, None, None, 100)
        .await
        .unwrap()
        .chat;
    let reply = service
        .admin_send(
            admin(users[2], true),
            chat.id,
            "PRIVATE REPLY",
            None,
            "mail-source-fixture",
        )
        .await
        .unwrap();
    let owned = intent(reply.id, users[0]);
    assert!(repo.owns_mail_reply(owned).await.unwrap());
    assert!(!repo
        .owns_mail_reply(intent(reply.id, users[1]))
        .await
        .unwrap());
    assert!(!repo
        .owns_mail_reply(intent(initial.id, users[0]))
        .await
        .unwrap());
    assert!(!repo
        .owns_mail_reply(MailIntent {
            purpose: MailPurpose::ProjectNews,
            ..owned
        })
        .await
        .unwrap());

    let guest = Requester::Guest(guest(users[1]));
    service.send(&guest, "guest question", None).await.unwrap();
    let guest_chat = service
        .messages(&guest, None, None, 100)
        .await
        .unwrap()
        .chat;
    let guest_reply = service
        .admin_send(
            admin(users[2], true),
            guest_chat.id,
            "guest reply",
            None,
            "mail-guest-fixture",
        )
        .await
        .unwrap();
    assert!(!repo
        .owns_mail_reply(intent(guest_reply.id, users[1]))
        .await
        .unwrap());

    // Removing the current account eliminates its source.
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(users[0])
        .execute(&pool)
        .await
        .unwrap();
    assert!(!repo.owns_mail_reply(owned).await.unwrap());
    sqlx::query("DELETE FROM support_conversations WHERE id=$1")
        .bind(guest_chat.id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=ANY($1)")
        .bind(&users[1..])
        .execute(&pool)
        .await
        .unwrap();
}
