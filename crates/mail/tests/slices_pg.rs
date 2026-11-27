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

#[tokio::test]
#[ignore = "requires disposable tor_ci PostgreSQL with migrations applied"]
async fn mail_01_deferred_reference_survives_restart_without_reusing_generation() {
    use tor_mail::domain::dispatch::{DeferOutcome, DeferReason, MAX_PRE_SUBMISSION_CLAIMS};
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let options: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
    assert_eq!(options.get_host(), "127.0.0.1");
    assert_eq!(options.get_database(), Some("tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Mail defer fixture','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("mail-defer-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let intent = MailIntent {
        purpose: MailPurpose::SupportReply,
        event_id: PositiveId::new(user).unwrap(),
        recipient: UserId::new(user).unwrap(),
    };
    let queue = PgMailQueue::new(pool.clone());
    assert!(queue.enqueue(intent).await.unwrap());
    let first = queue.claim_next().await.unwrap().unwrap();
    assert_eq!(first.intent, intent);
    assert_eq!(
        queue
            .defer_unsubmitted(first, DeferReason::DnsUnavailable)
            .await
            .unwrap(),
        DeferOutcome::Deferred
    );
    let restarted = PgMailQueue::new(PgPool::connect(&url).await.unwrap());
    assert!(restarted.claim_next().await.unwrap().is_none());
    assert!(!queue.finish(first, MailOutcome::Accepted).await.unwrap());
    assert!(!queue.enqueue(intent).await.unwrap());
    for generation in 2..=MAX_PRE_SUBMISSION_CLAIMS {
        sqlx::query("UPDATE mail_outbox SET not_before=clock_timestamp() WHERE id=$1")
            .bind(first.id)
            .execute(&pool)
            .await
            .unwrap();
        let next = restarted.claim_next().await.unwrap().unwrap();
        assert_eq!(next.id, first.id);
        assert_eq!(next.generation, generation);
        assert!(!queue.finish(first, MailOutcome::Accepted).await.unwrap());
        assert_eq!(
            restarted
                .defer_unsubmitted(next, DeferReason::DnsUnavailable)
                .await
                .unwrap(),
            if generation == MAX_PRE_SUBMISSION_CLAIMS {
                DeferOutcome::Exhausted
            } else {
                DeferOutcome::Deferred
            }
        );
    }
    assert!(restarted.claim_next().await.unwrap().is_none());
    let (state, reason): (String, String) =
        sqlx::query_as("SELECT status,defer_reason FROM mail_outbox WHERE id=$1")
            .bind(first.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(state, "failed");
    assert_eq!(reason, "dns_unavailable");

    let second = MailIntent {
        event_id: PositiveId::new(user + 1).unwrap(),
        ..intent
    };
    assert!(queue.enqueue(second).await.unwrap());
    let claim = queue.claim_next().await.unwrap().unwrap();
    // Simulate an already committed reservation without contacting SMTP.
    sqlx::query(
        "INSERT INTO mail_send_reservations
         (outbox_id,generation,purpose,recipient_key,reserved_at)
         VALUES($1,$2,'support_reply',$3,clock_timestamp())",
    )
    .bind(claim.id)
    .bind(claim.generation)
    .bind([13u8; 32].as_slice())
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        queue
            .defer_unsubmitted(claim, DeferReason::ProviderUnavailable)
            .await
            .unwrap(),
        DeferOutcome::Refused
    );
    assert!(queue.finish(claim, MailOutcome::Uncertain).await.unwrap());
    sqlx::query("DELETE FROM mail_send_reservations WHERE outbox_id=$1")
        .bind(claim.id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
}
