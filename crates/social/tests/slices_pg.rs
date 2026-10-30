//! Disposable PostgreSQL only. These exercise the same service as HTML/JSON.
use sqlx::PgPool;
use std::time::{SystemTime, UNIX_EPOCH};
use tor_social::{
    domain::MailError,
    repositories::{
        friendships::{FriendState, PgFriendships},
        mail::PgMail,
        notifications::PgNotifications,
    },
    services::MailService,
};

async fn fixture() -> (PgPool, Vec<i64>) {
    let url = std::env::var("TEST_DATABASE_URL").expect("disposable PG");
    assert!(url.ends_with("/tor_ci"), "live database forbidden");
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut users = Vec::new();
    for i in 0..3 {
        users.push(sqlx::query_scalar("INSERT INTO users(email_normalized,display_name,password_hash) VALUES($1,'Mail fixture','$argon2id$fixture') RETURNING id")
            .bind(format!("mail-{nonce}-{i}@example.invalid")).fetch_one(&pool).await.unwrap());
    }
    (pool, users)
}
async fn cleanup(pool: &PgPool, users: &[i64]) {
    sqlx::query("DELETE FROM users WHERE id=ANY($1)")
        .bind(users)
        .execute(pool)
        .await
        .unwrap();
}
fn key(n: usize) -> String {
    format!("{n:043}")
}

#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_membership_concurrent_replay_and_reconnect() {
    let (pool, u) = fixture().await;
    let s = MailService::new(PgMail::new(pool.clone()));
    let (a, b, c) = (u[0], u[1], u[2]);
    let (one, two) = tokio::join!(s.start(a, b), s.start(b, a));
    let id = one.unwrap();
    assert_eq!(id, two.unwrap());
    assert_eq!(s.start(a, a).await, Err(MailError::Invalid));
    assert_eq!(s.start(a, i64::MAX).await, Err(MailError::NotFound));
    let mut tasks = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let store = PgMail::new(pool.clone());
        tasks.spawn(async move {
            MailService::new(store)
                .send(a, id, "PRIVATE-MAIL-🐼", &key(1))
                .await
                .unwrap()
                .id
        });
    }
    let mut ids = Vec::new();
    while let Some(result) = tasks.join_next().await {
        ids.push(result.unwrap());
    }
    assert!(ids.iter().all(|n| *n == ids[0]));
    let message = ids[0];
    assert_eq!(
        s.send(a, id, "changed", &key(1)).await,
        Err(MailError::Conflict)
    );
    let other = s.start(a, c).await.unwrap();
    assert_eq!(
        s.send(a, other, "PRIVATE-MAIL-🐼", &key(1)).await,
        Err(MailError::Conflict)
    );
    assert_eq!(
        s.messages(c, id, None, None, 50).await,
        Err(MailError::NotFound)
    );
    assert_eq!(
        s.send(c, id, "intruder", &key(2)).await,
        Err(MailError::NotFound)
    );
    let next = s.send(a, id, "next", &key(3)).await.unwrap().id;
    let page = s.messages(b, id, Some(message), None, 1).await.unwrap();
    assert_eq!(page.items[0].id, next);
    assert!(!page.has_more);
    let restarted = MailService::new(PgMail::new(
        PgPool::connect(&std::env::var("TEST_DATABASE_URL").unwrap())
            .await
            .unwrap(),
    ));
    assert_eq!(
        restarted
            .messages(b, id, None, None, 50)
            .await
            .unwrap()
            .items
            .len(),
        2
    );
    assert!(
        sqlx::query("UPDATE social_conversations SET user_high=$1 WHERE id=$2")
            .bind(c.max(a))
            .bind(id)
            .execute(&pool)
            .await
            .is_err()
    );
    cleanup(&pool, &u).await;
}

#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_blocks_serialize_against_an_already_started_send() {
    let (pool, u) = fixture().await;
    let (a, b) = (u[0], u[1]);
    let s = MailService::new(PgMail::new(pool.clone()));
    let id = s.start(a, b).await.unwrap();
    let old = s.send(a, id, "history", &key(1)).await.unwrap().id;
    // Hold the exact block/send pair lock, commit a block, then let a sender
    // that already began its transaction resume. READ COMMITTED sees the block.
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('tor-social-pair:' || $1::BIGINT::TEXT || ':' || $2::BIGINT::TEXT,0))").bind(a.min(b)).bind(a.max(b)).execute(&mut *tx).await.unwrap();
    let sender = MailService::new(PgMail::new(pool.clone()));
    let pending = tokio::spawn(async move { sender.send(a, id, "raced", &key(2)).await });
    sqlx::query("INSERT INTO social_user_blocks(blocker_id,blocked_id) VALUES($1,$2)")
        .bind(b)
        .bind(a)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(pending.await.unwrap(), Err(MailError::Blocked));
    assert_eq!(
        s.send(b, id, "reverse blocked", &key(3)).await,
        Err(MailError::Blocked)
    );
    s.set_block(a, b, false).await.unwrap();
    assert_eq!(
        s.start(a, b).await,
        Err(MailError::Blocked),
        "cannot lift the other participant's block"
    );
    let page = s.messages(a, id, None, None, 50).await.unwrap();
    assert!(page.blocked);
    assert!(!page.blocked_by_me);
    assert_eq!(page.items[0].id, old);
    s.set_block(a, b, true).await.unwrap();
    s.set_block(b, a, false).await.unwrap();
    assert_eq!(
        s.send(b, id, "still blocked", &key(4)).await,
        Err(MailError::Blocked)
    );
    s.set_block(a, b, false).await.unwrap();
    s.send(b, id, "allowed again", &key(4)).await.unwrap();
    cleanup(&pool, &u).await;
}

#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_source_page_size_and_no_invented_send_limit() {
    let (pool, u) = fixture().await;
    let (a, b) = (u[0], u[1]);
    let s = MailService::new(PgMail::new(pool.clone()));
    let id = s.start(a, b).await.unwrap();
    // NEwBOO has no 30/min mail policy. More than 30 unique sends must work.
    for n in 0..31 {
        s.send(a, id, &format!("message {n}"), &key(n))
            .await
            .unwrap();
    }
    assert!(s.send(a, id, "message 0", &key(0)).await.unwrap().replayed);
    // A second sender fills a 56-message transcript, exceeding the source default of 50.
    for n in 0..25 {
        s.send(b, id, &format!("reply {n}"), &key(n)).await.unwrap();
    }
    let initial = s.messages(a, id, None, None, 50).await.unwrap();
    assert_eq!(initial.items.len(), 50);
    assert!(initial.has_more);
    let older = s
        .messages(a, id, None, Some(initial.items[0].id), 50)
        .await
        .unwrap();
    assert_eq!(older.items.len(), 6);
    assert!(!older.has_more);
    let mut cursor = 0;
    let mut all = Vec::new();
    loop {
        let p = s.messages(a, id, Some(cursor), None, 7).await.unwrap();
        for m in p.items {
            assert!(m.id > cursor);
            cursor = m.id;
            all.push(m.id);
        }
        if !p.has_more {
            break;
        }
    }
    assert_eq!(all.len(), 56);
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 56);
    assert!(s.messages(a, id, Some(0), Some(1), 50).await.is_err());
    // mail_service.py permits up to 100; a nonempty 2001-char body is valid.
    assert_eq!(
        s.messages(a, id, None, None, 100)
            .await
            .unwrap()
            .items
            .len(),
        56
    );
    assert!(s.messages(a, id, None, None, 101).await.is_err());
    s.send(a, id, &"я".repeat(2001), &key(2001)).await.unwrap();
    cleanup(&pool, &u).await;
}

#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_source_five_second_unkeyed_retry_is_sender_and_conversation_scoped() {
    // Oracle: NEwBOO test_mail_service_dedup.py and mail_service.py:193-250.
    let (pool, u) = fixture().await;
    let s = MailService::new(PgMail::new(pool.clone()));
    let (a, b, c) = (u[0], u[1], u[2]);
    let id = s.start(a, b).await.unwrap();
    let first = s.send_unkeyed(a, id, "same", &key(1)).await.unwrap();
    let replay = s.send_unkeyed(a, id, " same ", &key(2)).await.unwrap();
    assert_eq!(first.id, replay.id);
    assert!(replay.replayed);
    assert_ne!(
        s.send_unkeyed(a, id, "different", &key(3))
            .await
            .unwrap()
            .id,
        first.id
    );
    assert_ne!(
        s.send_unkeyed(b, id, "same", &key(4)).await.unwrap().id,
        first.id
    );
    let other = s.start(a, c).await.unwrap();
    assert_ne!(
        s.send_unkeyed(a, other, "same", &key(5)).await.unwrap().id,
        first.id
    );
    // Explicit IDs represent separate intents, even when text is the same.
    assert_ne!(s.send(a, id, "same", &key(6)).await.unwrap().id, first.id);
    // Seed an old row rather than sleeping or weakening the immutable-message guard.
    let old: i64 = sqlx::query_scalar("INSERT INTO social_messages(conversation_id,sender_id,body,command_key,created_at) VALUES($1,$2,'outside window',$3,clock_timestamp()-INTERVAL '6 seconds') RETURNING id")
        .bind(id).bind(a).bind(key(7)).fetch_one(&pool).await.unwrap();
    assert_ne!(
        s.send_unkeyed(a, id, "outside window", &key(8))
            .await
            .unwrap()
            .id,
        old
    );
    assert_eq!(
        s.send_unkeyed(c, id, "same", &key(9)).await,
        Err(MailError::NotFound)
    );
    s.set_block(b, a, true).await.unwrap();
    assert_eq!(
        s.send_unkeyed(a, id, "same", &key(10)).await,
        Err(MailError::Blocked)
    );
    cleanup(&pool, &u).await;
}

#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_source_mailbox_activity_preview_and_scoped_cursor() {
    // Source memory/mail_repository.py:69-101,180-183 and conversation_sidebar.html.
    let (pool, u) = fixture().await;
    let s = MailService::new(PgMail::new(pool.clone()));
    let (a, b, c) = (u[0], u[1], u[2]);
    let older = s.start(a, b).await.unwrap();
    let newer = s.start(a, c).await.unwrap();
    assert_eq!(s.mailbox(a, None, 1).await.unwrap()[0].id, newer);
    s.send(b, older, &"я".repeat(141), &key(1)).await.unwrap();
    let rows = s.mailbox(a, None, 1).await.unwrap();
    assert_eq!(
        rows[0].id, older,
        "new activity moves an existing conversation to the top"
    );
    assert_eq!(rows[0].last_message_preview, "я".repeat(140));
    assert!(!rows[0].last_message_at.is_empty());
    let next = s.mailbox(a, Some(older), 1).await.unwrap();
    assert_eq!(next[0].id, newer);
    assert!(next[0].last_message_preview.is_empty());
    assert!(
        s.mailbox(b, Some(newer), 100).await.unwrap().is_empty(),
        "foreign cursor cannot read another conversation's activity"
    );
    assert!(s
        .mailbox(c, None, 100)
        .await
        .unwrap()
        .iter()
        .all(|row| row.last_message_preview.is_empty()));
    // Source time_format_service uses fixed UTC+3, including midnight rollover.
    sqlx::query("INSERT INTO social_messages(conversation_id,sender_id,body,command_key,created_at) VALUES($1,$2,'MSK fixture',$3,'2026-10-30T22:30:00Z')")
        .bind(newer).bind(c).bind(key(2)).execute(&pool).await.unwrap();
    let page = s.messages(a, newer, None, None, 50).await.unwrap();
    assert_eq!(page.items[0].created_at_display, "31.10.2026 01:30");
    let rows = s.mailbox(a, None, 100).await.unwrap();
    assert_eq!(
        rows.iter()
            .find(|row| row.id == newer)
            .unwrap()
            .last_message_at,
        "31.10.2026 01:30"
    );
    cleanup(&pool, &u).await;
}

/// NEwBOO friend request: reciprocal intent accepts, block supersedes friendship.
#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_source_friendship_cross_request_and_block_serialization() {
    let (pool, u) = fixture().await;
    let (a, b) = (u[0], u[1]);
    let friends = PgFriendships::new(pool.clone());
    let mail = MailService::new(PgMail::new(pool.clone()));
    assert_eq!(friends.state(a, b).await.unwrap(), FriendState::None);
    let (one, two) = tokio::join!(friends.request(a, b), friends.request(b, a));
    assert!(one.is_ok());
    assert!(two.is_ok());
    assert_eq!(friends.state(a, b).await.unwrap(), FriendState::Friends);
    assert_eq!(friends.state(b, a).await.unwrap(), FriendState::Friends);
    assert_eq!(friends.request(a, b).await.unwrap(), FriendState::Friends);
    friends.remove(b, a).await.unwrap();
    assert_eq!(friends.state(a, b).await.unwrap(), FriendState::None);
    assert_eq!(friends.request(a, b).await.unwrap(), FriendState::Outgoing);
    assert_eq!(friends.state(b, a).await.unwrap(), FriendState::Incoming);
    mail.set_block(b, a, true).await.unwrap();
    assert_eq!(friends.state(a, b).await.unwrap(), FriendState::BlockedMe);
    assert_eq!(friends.state(b, a).await.unwrap(), FriendState::BlockedByMe);
    assert_eq!(friends.request(a, b).await, Err(MailError::Blocked));
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM social_friendships WHERE user_low=$1 AND user_high=$2",
    )
    .bind(a.min(b))
    .bind(a.max(b))
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        left, 0,
        "blocking deletes any existing accepted or pending friendship"
    );
    mail.set_block(a, b, false).await.unwrap();
    assert_eq!(friends.request(a, b).await, Err(MailError::Blocked));
    mail.set_block(b, a, false).await.unwrap();
    assert_eq!(friends.state(a, b).await.unwrap(), FriendState::None);
    cleanup(&pool, &u).await;
}

/// Outbox messages and friendship notifications: real PG, replay, block,
/// exactly-once on-site projection and actor-owned acknowledgement.
#[tokio::test]
#[ignore = "requires isolated TEST_DATABASE_URL ending /tor_ci"]
async fn e_08_notifications_outbox_once_on_replay_and_block() {
    let (pool, u) = fixture().await;
    let (a, b, c) = (u[0], u[1], u[2]);
    for (id, name) in [(a, "Alice"), (b, "Bob"), (c, "Carol")] {
        sqlx::query("UPDATE users SET display_name=$2 WHERE id=$1")
            .bind(id).bind(name).execute(&pool).await.unwrap();
    }
    let mail = MailService::new(PgMail::new(pool.clone()));
    let friends = PgFriendships::new(pool.clone());
    let notifications = PgNotifications::new(pool.clone());
    let conversation = mail.start(a, b).await.unwrap();
    let sent = mail.send(a, conversation, "CANONICAL-SECRET", &key(71)).await.unwrap();
    assert!(!sent.replayed);
    assert!(mail.send(a, conversation, "CANONICAL-SECRET", &key(71))
        .await.unwrap().replayed);
    let queued: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox_events
         WHERE event_type='social.mail.created'
           AND (payload->>'actor_user_id')::bigint=$1
           AND (payload->>'recipient_user_id')::bigint=$2",
    )
    .bind(a).bind(b).fetch_one(&pool).await.unwrap();
    assert_eq!(queued, 1, "a transport replay must not emit a second source fact");
    // Drain other fixture events too. Those belong to deleted test users and
    // are acknowledged without materializing orphan notifications.
    for _ in 0..12 {
        if notifications.deliver_pending(100).await.unwrap() < 100 { break; }
    }
    let inbox = notifications.list(b, None, 50).await.unwrap();
    let mail_rows: Vec<_> = inbox.iter().filter(|n| n.kind == "mail").collect();
    assert_eq!(mail_rows.len(), 1);
    assert_eq!(mail_rows[0].title, "Новое сообщение");
    assert_eq!(mail_rows[0].body, "Alice написал вам");
    assert_eq!(mail_rows[0].url, format!("/mail/{conversation}"));
    assert!(!mail_rows[0].body.contains("CANONICAL-SECRET"));
    assert!(notifications.list(a, None, 50).await.unwrap().is_empty());
    assert!(notifications.list(c, None, 50).await.unwrap().is_empty());
    assert_eq!(notifications.deliver_pending(1).await.unwrap(), 0);
    assert_eq!(notifications.mark_read(c, mail_rows[0].id).await.unwrap(), false);
    assert!(notifications.mark_read(b, mail_rows[0].id).await.unwrap());
    assert!(notifications.mark_read(b, mail_rows[0].id).await.unwrap());

    // The sender cannot send after block; a committed earlier message's
    // queued notification also cannot cross a subsequently committed block.
    mail.send(a, conversation, "AFTER FIRST", &key(72)).await.unwrap();
    mail.set_block(b, a, true).await.unwrap();
    notifications.deliver_pending(100).await.unwrap();
    assert_eq!(notifications.list(b, None, 50).await.unwrap().len(), 1);
    assert_eq!(mail.send(a, conversation, "blocked", &key(73)).await,
        Err(MailError::Blocked));
    mail.set_block(b, a, false).await.unwrap();

    // Friendship source events are transition-based, never per HTTP retry.
    assert_eq!(friends.request(a, c).await.unwrap(), FriendState::Outgoing);
    assert_eq!(friends.request(a, c).await.unwrap(), FriendState::Outgoing);
    notifications.deliver_pending(100).await.unwrap();
    let request = notifications.list(c, None, 50).await.unwrap();
    assert_eq!(request.len(), 1);
    assert_eq!(request[0].title, "Новая заявка в друзья");
    assert_eq!(request[0].url, "/users/Alice");
    assert_eq!(friends.request(c, a).await.unwrap(), FriendState::Friends);
    assert_eq!(friends.request(c, a).await.unwrap(), FriendState::Friends);
    notifications.deliver_pending(100).await.unwrap();
    let accept = notifications.list(a, None, 50).await.unwrap();
    assert_eq!(accept.len(), 1);
    assert_eq!(accept[0].title, "Заявка в друзья принята");
    friends.remove(a, c).await.unwrap();
    friends.remove(a, c).await.unwrap();
    notifications.deliver_pending(100).await.unwrap();
    let removed = notifications.list(c, None, 50).await.unwrap();
    assert_eq!(removed.len(), 2);
    assert_eq!(removed[0].title, "Удаление из друзей");
    assert_eq!(notifications.mark_all_read(c).await.unwrap(), 2);
    assert!(notifications.list(c, None, 50).await.unwrap().iter().all(|n| n.is_read));
    cleanup(&pool, &u).await;
}
