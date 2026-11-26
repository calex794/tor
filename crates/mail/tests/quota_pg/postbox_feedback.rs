use super::*;
use serde_json::json;
use tor_mail::{
    domain::{
        postbox::PostboxEvent,
        suppression::{AttemptRef, FeedbackDecision, SuppressionReason},
    },
    repositories::postbox_feedback::{PgPostboxFeedback, PostboxDecision},
};

fn complaint(
    event_id: &str,
    identity: &str,
    message: &str,
    marker: &str,
    email: &str,
) -> PostboxEvent {
    let payload = json!({
        "eventType":"Complaint",
        "eventId":event_id,
        "mail":{
            "identityId":identity,
            "messageId":message,
            "commonHeaders":{"originalMessageId":marker}
        },
        "complaint":{
            "complaintFeedbackType":"abuse",
            "complainedRecipients":[{"emailAddress":email}]
        }
    });
    PostboxEvent::parse(&serde_json::to_vec(&payload).unwrap())
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires migrated disposable local tor_ci; never use production"]
async fn mail_01_g2a_late_feedback_correlates_after_quota_pruning_and_restart() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let options: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
    assert_eq!(options.get_host(), "127.0.0.1");
    assert_eq!(options.get_database(), Some("tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let queue = PgMailQueue::new(pool.clone());
    let quota = PgMailQuota::new(pool.clone());
    let feedback = PgPostboxFeedback::new(pool.clone());
    clear(&pool).await;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let address = format!("mail-g2-{nonce}@example.invalid");
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash) \
         VALUES($1,'Mail correlation fixture','$argon2id$fixture') RETURNING id",
    )
    .bind(&address)
    .fetch_one(&pool)
    .await
    .unwrap();
    let user = UserId::new(user).unwrap();
    let signer = QuotaKeySigner::from_hex(&"4f".repeat(32)).unwrap();
    let key = signer.keys(&address, None).unwrap();
    let first = claim(&queue, user, 1, MailPurpose::SupportReply).await;
    let identity = format!("identity-{nonce}");
    let message = format!("message-{nonce}");
    assert!(feedback.prepare(first, &identity).await.unwrap().is_none());
    assert_eq!(
        quota.reserve(first, key).await.unwrap(),
        QuotaDecision::Reserved
    );
    let marker = feedback.prepare(first, &identity).await.unwrap().unwrap();
    assert!(feedback.prepare(first, &identity).await.unwrap().is_none());
    assert!(queue.finish(first, MailOutcome::Uncertain).await.unwrap());
    let attempt = AttemptRef {
        outbox_id: first.id,
        generation: first.generation,
    };
    sqlx::query("UPDATE mail_feedback_attempts SET reserved_at=clock_timestamp()-interval '10 days' WHERE outbox_id=$1")
        .bind(first.id)
        .execute(&pool)
        .await
        .unwrap();
    clear(&pool).await;
    let restarted = PgPostboxFeedback::new(PgPool::connect(&url).await.unwrap());

    for event in [
        complaint("wrong-identity", "foreign", &message, &marker, &address),
        complaint("wrong-recipient", &identity, &message, &marker, "other@example.invalid"),
        complaint("missing-marker", &identity, &message, "", &address),
    ] {
        assert_eq!(
            restarted.record_verified_event(&event, &signer).await.unwrap(),
            PostboxDecision::Uncorrelated
        );
    }
    let good = complaint("event-1", &identity, &message, &marker, &address);
    assert_eq!(
        restarted.record_verified_event(&good, &signer).await.unwrap(),
        PostboxDecision::Feedback(FeedbackDecision::Applied)
    );
    assert_eq!(
        restarted.record_verified_event(&good, &signer).await.unwrap(),
        PostboxDecision::Duplicate
    );
    assert!(feedback
        .bind_accepted_message(attempt, &identity, &message)
        .await
        .unwrap());
    assert!(!feedback
        .bind_accepted_message(attempt, &identity, "conflict")
        .await
        .unwrap());
    let wrong_message = complaint("event-2", &identity, "conflict", &marker, &address);
    assert_eq!(
        feedback.record_verified_event(&wrong_message, &signer).await.unwrap(),
        PostboxDecision::Uncorrelated
    );
    let by_receipt = complaint("event-3", &identity, &message, "", &address);
    assert_eq!(
        feedback.record_verified_event(&by_receipt, &signer).await.unwrap(),
        PostboxDecision::Feedback(FeedbackDecision::Unchanged)
    );
    let next = claim(&queue, user, 2, MailPurpose::SupportReply).await;
    assert_eq!(
        quota.reserve(next, key).await.unwrap(),
        QuotaDecision::RecipientSuppressed(SuppressionReason::Complaint)
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM mail_send_reservations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 0);
    sqlx::query("UPDATE mail_feedback_attempts SET reserved_at=clock_timestamp()-interval '91 days' WHERE outbox_id=$1")
        .bind(first.id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(feedback.prune_expired().await.unwrap(), 1);
    assert_eq!(
        feedback.record_verified_event(&good, &signer).await.unwrap(),
        PostboxDecision::Uncorrelated
    );
    sqlx::query("DELETE FROM mail_recipient_suppression WHERE recipient_key=$1")
        .bind(key.recipient().as_slice())
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user.get())
        .execute(&pool)
        .await
        .unwrap();
}
