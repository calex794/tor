//! Disposable real-PostgreSQL acceptance: no provider calls.
use super::*;
use tor_mail::{
    domain::{
        launch::{LaunchSnapshot, OperatorTestGrant},
        quota::{DAILY_LIMIT, TRANSACTIONAL_RESERVE},
        suppression::SuppressionReason,
    },
    repositories::launch::{OperatorTestReserveError, PgMailLaunch},
    services::launch::{LaunchError, LaunchFence},
};

pub(super) async fn exercise(
    pool: &PgPool,
    launch: &PgMailLaunch,
    fence: &LaunchFence,
    grant: &OperatorTestGrant,
    recipient: &str,
    before: &LaunchSnapshot,
) {
    use OperatorTestReserveError::{Profile, Quota};
    use QuotaDecision::*;
    let signer = QuotaKeySigner::from_hex(&"4f".repeat(32)).unwrap();
    let keys = signer.keys(recipient, None).unwrap();
    // CI-only binding; production never binds a key over existing history.
    sqlx::query(
        "UPDATE mail_send_budget_guard SET key_binding=$1 WHERE singleton AND key_binding IS NULL",
    )
    .bind(keys.signup_source().key_binding.as_slice())
    .execute(pool)
    .await
    .unwrap();
    assert!(matches!(
        launch
            .reserve_operator_test(grant, "foreign@example.invalid", &signer)
            .await,
        Err(Profile(ProfileError::Forbidden))
    ));
    let wrong = QuotaKeySigner::from_hex(&"5a".repeat(32)).unwrap();
    assert!(matches!(
        launch.reserve_operator_test(grant, recipient, &wrong).await,
        Err(Quota(KeyMismatch))
    ));
    sqlx::query(
        "INSERT INTO mail_recipient_suppression(recipient_key,reason,source_reserved_at) \
         VALUES($1,'complaint',clock_timestamp())",
    )
    .bind(keys.recipient().as_slice())
    .execute(pool)
    .await
    .unwrap();
    assert!(matches!(
        launch
            .reserve_operator_test(grant, recipient, &signer)
            .await,
        Err(Quota(RecipientSuppressed(SuppressionReason::Complaint)))
    ));
    sqlx::query("DELETE FROM mail_recipient_suppression WHERE recipient_key=$1")
        .bind(keys.recipient().as_slice())
        .execute(pool)
        .await
        .unwrap();
    seed(
        pool,
        DAILY_LIMIT - TRANSACTIONAL_RESERVE,
        MailPurpose::ProjectNews,
        super::super::keys(19, None),
        180,
    )
    .await;
    assert!(matches!(
        launch
            .reserve_operator_test(grant, recipient, &signer)
            .await,
        Err(Quota(TransactionalReserve))
    ));
    clear(pool).await;
    assert!(launch.snapshot().await.unwrap().unwrap() == *before);
    let attempts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM mail_outbox WHERE purpose='operator_test' AND event_id=$1",
    )
    .bind(before.policy_revision())
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(attempts, 0);
    // Two concurrent reservations for the same grant, one committed charge.
    let (a, b) = tokio::join!(
        launch.reserve_operator_test(grant, recipient, &signer),
        launch.reserve_operator_test(grant, recipient, &signer)
    );
    let receipt = match (a, b) {
        (Ok(value), Err(Profile(ProfileError::Conflict)))
        | (Err(Profile(ProfileError::Conflict)), Ok(value)) => value,
        _ => panic!("expected exactly one committed test reservation"),
    };
    let row: (String, i32, Vec<u8>, i64, Vec<u8>) = sqlx::query_as(
        "SELECT m.status,m.generation,r.recipient_key,m.event_id,r.launch_intent_digest \
         FROM mail_outbox m JOIN mail_send_reservations r \
         ON r.outbox_id=m.id AND r.generation=m.generation \
         WHERE m.id=$1 AND m.purpose='operator_test' AND r.purpose=m.purpose",
    )
    .bind(receipt.outbox_id())
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(row.0, "leased");
    assert_eq!(row.1, receipt.generation());
    assert_eq!(row.2, keys.recipient().to_vec());
    assert_eq!(row.3, before.policy_revision());
    assert_eq!(row.4.len(), 32);
    let after = launch.snapshot().await.unwrap().unwrap();
    assert_eq!(
        after.review_cost().usage.unwrap().reserved_attempts,
        before.review_cost().usage.unwrap().reserved_attempts + 1
    );
    // Final preparation must not trust the earlier charge alone.
    assert!(matches!(
        launch
            .prepare_operator_test(grant, &receipt, "foreign@example.invalid", &signer)
            .await,
        Err(ProfileError::Forbidden)
    ));
    assert!(matches!(
        launch
            .prepare_operator_test(grant, &receipt, recipient, &wrong)
            .await,
        Err(ProfileError::Conflict)
    ));
    sqlx::query(
        "INSERT INTO mail_recipient_suppression(recipient_key,reason,source_reserved_at) \
         VALUES($1,'complaint',clock_timestamp())",
    )
    .bind(keys.recipient().as_slice())
    .execute(pool)
    .await
    .unwrap();
    assert!(matches!(
        launch
            .prepare_operator_test(grant, &receipt, recipient, &signer)
            .await,
        Err(ProfileError::Conflict)
    ));
    sqlx::query("DELETE FROM mail_recipient_suppression WHERE recipient_key=$1")
        .bind(keys.recipient().as_slice())
        .execute(pool)
        .await
        .unwrap();
    let start = launch
        .prepare_operator_test(grant, &receipt, recipient, &signer)
        .await
        .unwrap();
    assert_eq!(start.recipient(), recipient);
    let marker = start.marker().to_owned();
    assert!(marker.starts_with("<tor-"));
    let completion = fence.consume_operator_test_start(start).unwrap();
    assert_eq!(
        fence.precheck_operator_test(grant, before),
        Err(LaunchError::Stopped)
    );
    assert!(matches!(
        launch
            .prepare_operator_test(grant, &receipt, recipient, &signer)
            .await,
        Err(ProfileError::Conflict)
    ));
    // Simulate a positive SMTP acknowledgement; no real provider call.
    assert!(launch
        .finish_operator_test(&completion, MailOutcome::Accepted, Some("ci-test-receipt"))
        .await
        .unwrap());
    assert!(!launch
        .finish_operator_test(&completion, MailOutcome::Uncertain, None)
        .await
        .unwrap());
    let result: (String, String, String) = sqlx::query_as(
        "SELECT m.status,f.marker,f.provider_message_id FROM mail_outbox m \
         JOIN mail_feedback_attempts f ON f.outbox_id=m.id AND f.generation=m.generation \
         WHERE m.id=$1",
    )
    .bind(receipt.outbox_id())
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(result.0, "accepted");
    assert_eq!(format!("<tor-{}@pandaolate.ru>", result.1), marker);
    assert_eq!(result.2, "ci-test-receipt");
    assert!(launch.snapshot().await.unwrap().unwrap() == after);
    // Expired test leases remain terminal uncertain, never pending/deferred.
    // Isolated fixture row only; no charge or SMTP is associated with it.
    let expired: i64 = sqlx::query_scalar(
        "INSERT INTO mail_outbox \
         (purpose,event_id,recipient_user_id,status,generation,lease_until) \
         VALUES('operator_test',900000000000,$1,'leased',1,clock_timestamp()-interval '1 second') \
         RETURNING id",
    )
    .bind(grant.actor())
    .fetch_one(pool)
    .await
    .unwrap();
    PgMailQueue::new(pool.clone())
        .recover_expired()
        .await
        .unwrap();
    let status: String = sqlx::query_scalar("SELECT status FROM mail_outbox WHERE id=$1")
        .bind(expired)
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(status, "uncertain");
    assert!(launch.snapshot().await.unwrap().unwrap() == after);
}
