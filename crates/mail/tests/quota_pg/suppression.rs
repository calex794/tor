use super::*;
use tor_mail::{
    domain::suppression::{AttemptRef, DeliveryFeedback, FeedbackDecision, SuppressionReason},
    repositories::suppression::PgMailSuppression,
};

#[tokio::test]
#[ignore = "requires migrated disposable local tor_ci; never use production"]
async fn mail_01_g1_suppression_is_durable_correlated_and_precedes_quota() {
    use DeliveryFeedback::*;
    use FeedbackDecision::*;
    use MailPurpose::*;

    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let options: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
    assert_eq!(options.get_host(), "127.0.0.1");
    assert_eq!(options.get_database(), Some("tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    configure_cost_fixture(&pool).await;
    let queue = PgMailQueue::new(pool.clone());
    let quota = PgMailQuota::new(pool.clone());
    let suppression = PgMailSuppression::new(pool.clone());
    clear(&pool).await;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    async fn user(pool: &PgPool, email: String) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash) \
             VALUES($1,'Mail suppression fixture','$argon2id$fixture') RETURNING id",
        )
        .bind(email)
        .fetch_one(pool)
        .await
        .unwrap()
    }
    let first_user =
        UserId::new(user(&pool, format!("mail-g1-{nonce}@example.invalid")).await).unwrap();
    let signer = QuotaKeySigner::from_hex(&"4f".repeat(32)).unwrap();
    let address = format!("mail-g1-{nonce}@example.invalid");
    let key = signer.keys(&address, None).unwrap();

    let first = claim(&queue, first_user, 1, SupportReply).await;
    assert_eq!(
        quota.reserve(first, key).await.unwrap(),
        QuotaDecision::Reserved
    );
    assert!(queue.finish(first, MailOutcome::Accepted).await.unwrap());
    let attempt = AttemptRef {
        outbox_id: first.id,
        generation: first.generation,
    };
    assert_eq!(
        suppression
            .record_verified_feedback(attempt, TemporaryFailure)
            .await
            .unwrap(),
        Ignored
    );
    assert_eq!(
        suppression
            .record_verified_feedback(attempt, OtherPermanentFailure)
            .await
            .unwrap(),
        Ignored
    );
    assert_eq!(
        suppression
            .record_verified_feedback(
                AttemptRef {
                    generation: attempt.generation + 1,
                    ..attempt
                },
                SpamComplaint,
            )
            .await
            .unwrap(),
        Uncorrelated
    );
    assert_eq!(
        suppression
            .record_verified_feedback(attempt, RecipientDoesNotExist)
            .await
            .unwrap(),
        Applied
    );
    assert_eq!(
        suppression
            .record_verified_feedback(attempt, RecipientDoesNotExist)
            .await
            .unwrap(),
        Unchanged
    );

    // Delete the first account and use a new account with the same address.
    // The stop-list and the attempt are keyed by mailbox, not account ID.
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(first_user.get())
        .execute(&pool)
        .await
        .unwrap();
    let second_user = UserId::new(user(&pool, address.clone()).await).unwrap();
    let second = claim(&queue, second_user, 2, ProjectNews).await;
    let restart_pool = PgPool::connect(&url).await.unwrap();
    let restarted = PgMailQuota::new(restart_pool.clone());
    let restarted_suppression = PgMailSuppression::new(restart_pool);
    let equivalent = signer.keys(&address.to_ascii_uppercase(), None).unwrap();
    assert_eq!(
        restarted.reserve(second, equivalent).await.unwrap(),
        QuotaDecision::RecipientSuppressed(SuppressionReason::HardBounce)
    );
    assert_eq!(
        restarted_suppression
            .record_verified_feedback(attempt, SpamComplaint)
            .await
            .unwrap(),
        Applied
    );
    for feedback in [RecipientDoesNotExist, SpamComplaint] {
        assert_eq!(
            suppression
                .record_verified_feedback(attempt, feedback)
                .await
                .unwrap(),
            Unchanged
        );
    }
    assert_eq!(
        suppression
            .record_verified_feedback(attempt, Delivered)
            .await
            .unwrap(),
        Ignored
    );
    assert_eq!(
        restarted.reserve(second, key).await.unwrap(),
        QuotaDecision::RecipientSuppressed(SuppressionReason::Complaint)
    );

    let registration = claim(&queue, second_user, 3, RegistrationConfirmation).await;
    let registration_key = signer
        .keys_for_registration(
            registration.intent,
            &address,
            RegistrationMailEvidence {
                user_id: second_user,
                source: key.signup_source(),
            },
        )
        .unwrap();
    assert_eq!(
        quota.reserve(registration, registration_key).await.unwrap(),
        QuotaDecision::RecipientSuppressed(SuppressionReason::Complaint)
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM mail_send_reservations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "blocked attempts must not spend quota");

    // Expired bounce entries stop blocking and are pruned, not complaints.
    let other_address = format!("mail-g1-other-{nonce}@example.invalid");
    let other_key = signer.keys(&other_address, None).unwrap();
    sqlx::query(
        "INSERT INTO mail_recipient_suppression \
         (recipient_key,reason,source_reserved_at,suppressed_until) VALUES \
         ($1,'hard_bounce',clock_timestamp()-interval '31 days',clock_timestamp()-interval '1 day')",
    )
    .bind(other_key.recipient().as_slice())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE mail_send_reservations SET reserved_at=clock_timestamp()-interval '2 seconds'",
    )
    .execute(&pool)
    .await
    .unwrap();
    let other_user = UserId::new(user(&pool, other_address).await).unwrap();
    let other = claim(&queue, other_user, 4, SupportReply).await;
    assert_eq!(
        quota.reserve(other, other_key).await.unwrap(),
        QuotaDecision::Reserved
    );
    assert_eq!(suppression.prune_expired().await.unwrap(), 1);
    assert_eq!(
        restarted.reserve(second, key).await.unwrap(),
        QuotaDecision::RecipientSuppressed(SuppressionReason::Complaint)
    );

    for fixture in [second_user, other_user] {
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(fixture.get())
            .execute(&pool)
            .await
            .unwrap();
    }
    sqlx::query("DELETE FROM mail_recipient_suppression WHERE recipient_key=$1")
        .bind(key.recipient().as_slice())
        .execute(&pool)
        .await
        .unwrap();
    clear(&pool).await;
}
