use sqlx::PgPool;
use std::time::{SystemTime, UNIX_EPOCH};
use tor_contracts::auth::{Actor, SessionActor, UserId};
use tor_support::{
    domain::{Requester, SupportError},
    repositories::postgres::PgSupport,
    services::SupportService,
};

async fn fixture() -> (PgPool, Vec<i64>) {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut users = Vec::new();
    for i in 0..3 {
        users.push(sqlx::query_scalar("INSERT INTO users(email_normalized,display_name,password_hash) VALUES($1,'Support fixture','$argon2id$fixture') RETURNING id")
            .bind(format!("support-{n}-{i}@example.invalid")).fetch_one(&pool).await.unwrap());
    }
    (pool, users)
}
fn admin(id: i64, allowed: bool) -> SessionActor {
    SessionActor {
        actor: Actor {
            user_id: UserId::new(id).unwrap(),
        },
        author: false,
        administrator: allowed,
    }
}
fn guest(user: i64) -> [u8; 32] {
    let mut h = [74; 32];
    h[..8].copy_from_slice(&user.to_be_bytes());
    h
}

#[tokio::test]
#[ignore = "isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_11_guest_claim_merges_histories_and_cannot_be_reassigned() {
    let (pool, u) = fixture().await;
    let repo = PgSupport::new(pool.clone());
    let service = SupportService::new(repo.clone());
    let hash = guest(u[0]);
    let g = Requester::Guest(hash);
    let a = Requester::User {
        id: u[0],
        guest: None,
    };
    let guest_message = service
        .send(&g, "GUEST SECRET", Some("same-key"))
        .await
        .unwrap();
    let user_message = service
        .send(&a, "EXISTING USER HISTORY", Some("same-key"))
        .await
        .unwrap();
    let claim = Requester::User {
        id: u[0],
        guest: Some(hash),
    };
    let page = service.messages(&claim, None, None, 100).await.unwrap();
    assert_eq!(page.messages.len(), 2);
    assert_eq!(page.messages[0].id, guest_message.id);
    assert_eq!(page.messages[1].id, user_message.id);
    let canonical = page.chat.id;
    let strict_key = "J".repeat(43);
    let first = service
        .json_send(&a, "JSON command", &strict_key)
        .await
        .unwrap();
    assert_eq!(
        service
            .json_send(&a, "JSON command", &strict_key)
            .await
            .unwrap()
            .id,
        first.id
    );
    assert_eq!(
        service
            .json_send(&a, "different payload", &strict_key)
            .await,
        Err(SupportError::Conflict)
    );

    assert_eq!(
        service
            .messages(&claim, None, None, 100)
            .await
            .unwrap()
            .chat
            .id,
        canonical
    );
    assert_eq!(
        service
            .messages(
                &Requester::User {
                    id: u[1],
                    guest: Some(hash)
                },
                None,
                None,
                100
            )
            .await
            .err(),
        Some(SupportError::Denied)
    );
    assert_eq!(
        service.messages(&g, None, None, 100).await.err(),
        Some(SupportError::Denied)
    );
    assert!(service
        .messages(
            &Requester::User {
                id: u[1],
                guest: None
            },
            None,
            None,
            100
        )
        .await
        .unwrap()
        .messages
        .is_empty());
    let restarted = SupportService::new(PgSupport::new(
        PgPool::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
            .await
            .unwrap(),
    ));
    assert_eq!(
        restarted
            .messages(&a, None, None, 100)
            .await
            .unwrap()
            .messages
            .len(),
        3
    );
    let expiries: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM support_conversations WHERE id=$1 AND expires_at IS NOT NULL",
    )
    .bind(canonical)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(expiries, 0);
    sqlx::query("DELETE FROM users WHERE id=ANY($1)")
        .bind(&u)
        .execute(&pool)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_11_concurrent_sends_claims_admin_cursor_and_expiry() {
    let (pool, u) = fixture().await;
    let repo = PgSupport::new(pool.clone());
    let service = SupportService::new(repo.clone());
    let hash = guest(u[0]);
    let g = Requester::Guest(hash);
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let repo = repo.clone();
        let g = g.clone();
        tasks.spawn(async move {
            SupportService::new(repo)
                .send(&g, "one canonical message", Some("retry"))
                .await
                .unwrap()
                .id
        });
    }
    let mut ids = Vec::new();
    while let Some(r) = tasks.join_next().await {
        ids.push(r.unwrap());
    }
    assert!(ids.iter().all(|n| *n == ids[0]));
    let chat = service.messages(&g, None, None, 100).await.unwrap().chat.id;
    assert_eq!(
        service
            .admin_messages(admin(u[2], false), chat, None, None, 100)
            .await
            .err(),
        Some(SupportError::Denied)
    );
    let list = service
        .admin_chats(admin(u[2], true), None, 200)
        .await
        .unwrap();
    assert!(list.iter().find(|c| c.id == chat).unwrap().unread);
    let page = service
        .admin_messages(admin(u[2], true), chat, None, None, 100)
        .await
        .unwrap();
    assert_eq!(page.messages.len(), 1);
    assert!(
        service
            .admin_chats(admin(u[2], true), None, 200)
            .await
            .unwrap()
            .iter()
            .find(|c| c.id == chat)
            .unwrap()
            .unread,
        "GET must not advance the administrative read cursor"
    );

    service
        .admin_mark_read(admin(u[2], true), chat, page.messages[0].id, "fixture-read")
        .await
        .unwrap();
    assert!(
        !service
            .admin_chats(admin(u[2], true), None, 200)
            .await
            .unwrap()
            .iter()
            .find(|c| c.id == chat)
            .unwrap()
            .unread
    );
    service
        .admin_mark_read(
            admin(u[2], true),
            chat,
            page.messages[0].id,
            "fixture-read-retry",
        )
        .await
        .unwrap();
    let read_audits: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM admin_action_audit WHERE actor_id=$1 AND action='support.read'",
    )
    .bind(u[2])
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(read_audits, 1);
    let reply = service
        .admin_send(
            admin(u[2], true),
            chat,
            "ADMIN REPLY",
            Some("retry"),
            "fixture-request",
        )
        .await
        .unwrap();
    assert_eq!(
        service
            .admin_send(
                admin(u[2], true),
                chat,
                "changed retry",
                Some("retry"),
                "fixture-request"
            )
            .await
            .unwrap()
            .id,
        reply.id
    );
    let reply_audits:i64=sqlx::query_scalar("SELECT count(*) FROM admin_action_audit WHERE actor_id=$1 AND action='support.reply' AND resource_id=$2")
        .bind(u[2]).bind(reply.id).fetch_one(&pool).await.unwrap();
    assert_eq!(reply_audits, 1);
    assert!(
        sqlx::query("UPDATE admin_action_audit SET outcome='changed' WHERE actor_id=$1")
            .bind(u[2])
            .execute(&pool)
            .await
            .is_err()
    );
    assert_eq!(
        service
            .messages(&g, Some(ids[0]), None, 100)
            .await
            .unwrap()
            .messages[0]
            .body,
        "ADMIN REPLY"
    );
    for i in 0..201 {
        service
            .send(&g, &format!("page {i}"), Some(&format!("page-{i}")))
            .await
            .unwrap();
    }
    let latest = service.messages(&g, None, None, 100).await.unwrap();
    assert_eq!(latest.messages.len(), 100);
    assert!(latest.has_more);
    let older = service
        .messages(&g, None, Some(latest.messages[0].id), 100)
        .await
        .unwrap();
    assert_eq!(older.messages.len(), 100);
    assert!(older.has_more);
    let oldest = service
        .messages(&g, None, Some(older.messages[0].id), 100)
        .await
        .unwrap();
    assert_eq!(oldest.messages.len(), 3);
    assert!(!oldest.has_more);
    // Two accounts race to claim the same capability. Exactly one receives it.
    let (one, two) = tokio::join!(
        service.messages(
            &Requester::User {
                id: u[0],
                guest: Some(hash)
            },
            None,
            None,
            100
        ),
        service.messages(
            &Requester::User {
                id: u[1],
                guest: Some(hash)
            },
            None,
            None,
            100
        )
    );
    assert_ne!(one.is_ok(), two.is_ok());
    assert!(matches!(
        one.err().or(two.err()),
        Some(SupportError::Denied)
    ));
    // An expired unrelated guest conversation and its private messages disappear.
    let mut expired = hash;
    expired[31] ^= 17;
    let who = Requester::Guest(expired);
    service
        .send(&who, "EXPIRED PRIVATE TEXT", Some("expired"))
        .await
        .unwrap();
    let expired_id = service
        .messages(&who, None, None, 100)
        .await
        .unwrap()
        .chat
        .id;
    sqlx::query(
        "UPDATE support_conversations SET expires_at=now()-INTERVAL '1 second' WHERE id=$1",
    )
    .bind(expired_id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        service
            .admin_messages(admin(u[2], true), expired_id, None, None, 100)
            .await
            .err(),
        Some(SupportError::NotFound)
    );
    assert!(repo.cleanup_expired(200).await.unwrap() > 0);
    let n: i64 =
        sqlx::query_scalar("SELECT count(*) FROM support_messages WHERE conversation_id=$1")
            .bind(expired_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(n, 0);
    sqlx::query("DELETE FROM users WHERE id=ANY($1)")
        .bind(&u)
        .execute(&pool)
        .await
        .unwrap();
}
