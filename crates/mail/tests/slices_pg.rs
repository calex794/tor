use sqlx::PgPool;
use std::time::{SystemTime, UNIX_EPOCH};
use tor_contracts::{
    auth::UserId,
    mail::{MailIntent, MailPurpose},
    paid_delivery::PositiveId,
};
use tor_mail::{domain::MailOutcome, repositories::PgMailQueue};

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL with migrations applied"]
async fn mail_01_duplicate_claim_fence_restart_and_priority() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    assert!(url.ends_with("/tor_ci") && url.contains("127.0.0.1"));
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash) \
         VALUES($1,'Mail fixture','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("mail-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let intent = |purpose| MailIntent {
        purpose,
        event_id: PositiveId::new(user).unwrap(),
        recipient: UserId::new(user).unwrap(),
    };
    let queue = PgMailQueue::new(pool.clone());
    let news = intent(MailPurpose::ProjectNews);
    let registration = intent(MailPurpose::RegistrationConfirmation);
    assert!(queue.enqueue(news).await.unwrap());
    let (a, b) = tokio::join!(queue.enqueue(registration), queue.enqueue(registration));
    assert_ne!(a.unwrap(), b.unwrap());
    let restarted = PgMailQueue::new(PgPool::connect(&url).await.unwrap());
    let first = restarted.claim_next().await.unwrap().unwrap();
    assert_eq!(first.intent, registration);
    let second = queue.claim_next().await.unwrap().unwrap();
    assert_eq!(second.intent, news);
    let mut stale = first;
    stale.generation += 1;
    assert!(!queue.finish(stale, MailOutcome::Accepted).await.unwrap());
    assert!(queue.finish(first, MailOutcome::Accepted).await.unwrap());
    assert!(!queue.finish(first, MailOutcome::Accepted).await.unwrap());
    sqlx::query("UPDATE mail_outbox SET lease_until=now()-interval '1 second' WHERE id=$1")
        .bind(second.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!queue.finish(second, MailOutcome::Accepted).await.unwrap());
    assert_eq!(restarted.recover_expired().await.unwrap(), 1);
    let state: String = sqlx::query_scalar("SELECT status FROM mail_outbox WHERE id=$1")
        .bind(second.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "uncertain");
    assert!(queue.claim_next().await.unwrap().is_none());
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
}

#[path = "quota_pg/mod.rs"]
mod quota_pg;
