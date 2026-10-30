//! Disposable PostgreSQL only. These exercise the same service as HTML/JSON.
use sqlx::PgPool;
use std::time::{SystemTime, UNIX_EPOCH};
use tor_social::{domain::MailError, repositories::mail::PgMail, services::MailService};

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
async fn e_08_membership_concurrent_replay_and_read_cursor() {
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
    assert_eq!(s.mark_read(c, id, message).await, Err(MailError::NotFound));
    assert_eq!(s.mailbox(b, None, 50).await.unwrap()[0].unread, 1);
    assert_eq!(
        s.mailbox(a, None, 50)
            .await
            .unwrap()
            .iter()
            .find(|r| r.id == id)
            .unwrap()
            .unread,
        0
    );
    for _ in 0..2 {
        s.mark_read(b, id, message).await.unwrap();
    }
    assert_eq!(s.mailbox(b, None, 50).await.unwrap()[0].unread, 0);
    let next = s.send(a, id, "next", &key(3)).await.unwrap().id;
    let page = s.messages(b, id, Some(message), None, 1).await.unwrap();
    assert_eq!(page.items[0].id, next);
    assert!(!page.has_more);
    s.mark_read(b, id, next).await.unwrap();
    s.mark_read(b, id, message).await.unwrap();
    assert_eq!(
        s.mailbox(b, None, 50).await.unwrap()[0].unread,
        0,
        "old device cannot rewind read state"
    );
    assert_eq!(s.mark_read(b, other, next).await, Err(MailError::NotFound));
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
async fn e_08_cursor_pages_and_distributed_rate_limit_do_not_lose_messages() {
    let (pool, u) = fixture().await;
    let (a, b) = (u[0], u[1]);
    let s = MailService::new(PgMail::new(pool.clone()));
    let id = s.start(a, b).await.unwrap();
    for n in 0..30 {
        s.send(a, id, &format!("message {n}"), &key(n))
            .await
            .unwrap();
    }
    assert_eq!(
        s.send(a, id, "too many", &key(31)).await,
        Err(MailError::RateLimited)
    );
    assert!(s.send(a, id, "message 0", &key(0)).await.unwrap().replayed);
    // A second sender fills a 55-message transcript, exceeding the default page.
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
    assert_eq!(older.items.len(), 5);
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
    assert_eq!(all.len(), 55);
    all.sort();
    all.dedup();
    assert_eq!(all.len(), 55);
    assert!(s.messages(a, id, Some(0), Some(1), 50).await.is_err());
    assert!(s.messages(a, id, None, None, 51).await.is_err());
    cleanup(&pool, &u).await;
}
