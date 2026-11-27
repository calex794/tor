use super::*;
use tor_mail::domain::dispatch::{DeferDecision, DeferReason};

#[tokio::test]
#[ignore = "requires migrated disposable tor_ci PostgreSQL"]
async fn mail_01_deferred_queue_is_bounded_and_never_replays_charged_claims() {
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
        "INSERT INTO users(email_normalized,display_name,password_hash) \
         VALUES($1,'Deferred fixture','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("deferred-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let intent = |purpose, event| MailIntent {
        purpose,
        event_id: PositiveId::new(event).unwrap(),
        recipient: UserId::new(user).unwrap(),
    };
    let queue = PgMailQueue::new(pool.clone());
    let purposes = [MailPurpose::PasswordReset];
    let news = intent(MailPurpose::ProjectNews, user);
    let reset = intent(MailPurpose::PasswordReset, user);
    assert!(queue.enqueue(news).await.unwrap());
    assert!(queue.claim_next_for(&purposes).await.unwrap().is_none());
    assert!(queue.enqueue(reset).await.unwrap());
    let first = queue.claim_next_for(&purposes).await.unwrap().unwrap();
    assert_eq!(first.intent, reset);
    let mut wrong = first;
    wrong.intent.event_id = PositiveId::new(user + 1).unwrap();
    assert_eq!(
        queue
            .defer_unreserved(wrong, DeferReason::DnsUnavailable)
            .await
            .unwrap(),
        DeferDecision::Stale
    );
    assert!(!queue.finish(wrong, MailOutcome::Cancelled).await.unwrap());
    assert_eq!(
        queue
            .defer_unreserved(first, DeferReason::DnsUnavailable)
            .await
            .unwrap(),
        DeferDecision::Deferred
    );
    let restarted = PgMailQueue::new(PgPool::connect(&url).await.unwrap());
    assert!(restarted.claim_next_for(&purposes).await.unwrap().is_none());
    assert!(!queue.enqueue(reset).await.unwrap());

    for generation in 2..=4 {
        sqlx::query(
            "UPDATE mail_outbox SET next_attempt_at=clock_timestamp()-interval '1 second' \
             WHERE id=$1 AND status='deferred'",
        )
        .bind(first.id)
        .execute(&pool)
        .await
        .unwrap();
        let claim = restarted.claim_next_for(&purposes).await.unwrap().unwrap();
        assert_eq!(claim.id, first.id);
        assert_eq!(claim.generation, generation);
        assert!(!queue.finish(first, MailOutcome::Accepted).await.unwrap());
        let expected = if generation == 4 {
            DeferDecision::Exhausted
        } else {
            DeferDecision::Deferred
        };
        assert_eq!(
            restarted
            .defer_unreserved(claim, DeferReason::GlobalPace)
            .await
            .unwrap(),
            expected
        );
    }
    assert!(queue.claim_next_for(&purposes).await.unwrap().is_none());
    let charged_intent = intent(MailPurpose::PasswordReset, user + 1);
    assert!(queue.enqueue(charged_intent).await.unwrap());
    let charged = queue.claim_next_for(&purposes).await.unwrap().unwrap();
    sqlx::query(
        "INSERT INTO mail_send_reservations(outbox_id,generation,purpose,recipient_key,reserved_at) \
         VALUES($1,$2,'password_reset',$3,clock_timestamp())",
    )
    .bind(charged.id)
    .bind(charged.generation)
    .bind([17_u8; 32].as_slice())
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        queue
            .defer_unreserved(charged, DeferReason::TransportUnavailable)
            .await
            .unwrap(),
        DeferDecision::Charged
    );
    sqlx::query(
        "UPDATE mail_outbox SET lease_until=clock_timestamp()-interval '1 second' WHERE id=$1",
    )
    .bind(charged.id)
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(restarted.recover_expired().await.unwrap(), 1);
    assert!(restarted.claim_next_for(&purposes).await.unwrap().is_none());
    let state: String = sqlx::query_scalar("SELECT status FROM mail_outbox WHERE id=$1")
        .bind(charged.id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(state, "uncertain");
    let other = queue.claim_next().await.unwrap().unwrap();
    assert_eq!(other.intent, news);
    assert!(queue.finish(other, MailOutcome::Cancelled).await.unwrap());
    sqlx::query("DELETE FROM mail_send_reservations WHERE outbox_id=$1")
        .bind(charged.id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
}
