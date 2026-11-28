//! Dispatch admission regression in the existing disposable Postbox fixture.
use super::*;
use sha2::{Digest, Sha256};
use tor_mail::{
    domain::{
        dispatch::{DeferOutcome, DeferReason},
        launch::LaunchSnapshot,
    },
    repositories::{
        cost::{admin::MonthReconciliation, PgMailCost},
        launch::PgMailLaunch,
        postbox_feedback::PgPostboxFeedback,
    },
    services::launch::{LaunchError, LaunchFence, LaunchScope},
};

async fn actor_keys(pool: &PgPool, actor: i64) -> QuotaKeys {
    let address: String = sqlx::query_scalar("SELECT email_normalized FROM users WHERE id=$1")
        .bind(actor)
        .fetch_one(pool)
        .await
        .unwrap();
    QuotaKeySigner::from_hex(&"4f".repeat(32))
        .unwrap()
        .keys(&address, None)
        .unwrap()
}

pub(super) async fn reject_test_scope(pool: &PgPool, actor: i64, active: &LaunchSnapshot) {
    let queue = PgMailQueue::new(pool.clone());
    let quota = PgMailQuota::new(pool.clone());
    let feedback = PgPostboxFeedback::new(pool.clone());
    let attempt = claim(
        &queue,
        UserId::new(actor).unwrap(),
        1,
        MailPurpose::SupportReply,
    )
    .await;
    let key = actor_keys(pool, actor).await;
    assert_eq!(
        quota
            .reserve_for_dispatch(attempt, key, active)
            .await
            .unwrap(),
        QuotaDecision::CostEmergencyStop
    );
    assert_eq!(
        quota.reserve(attempt, key).await.unwrap(),
        QuotaDecision::CostEmergencyStop
    );
    assert!(feedback
        .prepare_for_dispatch(attempt, "ci-identity", key, active)
        .await
        .unwrap()
        .is_none());
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM mail_send_reservations WHERE outbox_id=$1")
            .bind(attempt.id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    assert!(queue.finish(attempt, MailOutcome::Cancelled).await.unwrap());
}

async fn activate_dispatch(
    pool: &PgPool,
    digest: &[u8; 32],
    fence: &LaunchFence,
) -> LaunchSnapshot {
    let cost = PgMailCost::new(pool.clone());
    let launch = PgMailLaunch::new(pool.clone());
    let view = cost.admin_view(digest).await.unwrap();
    let reconciliation = MonthReconciliation {
        usage: view.snapshot.usage.unwrap(),
        evidence_sha256: [18; 32],
    };
    cost.admin_reconcile_month(digest, view.revision, &reconciliation)
        .await
        .unwrap();
    let stopped = launch.snapshot().await.unwrap().unwrap();
    let challenge = fence
        .begin_review(&stopped, LaunchScope::Dispatch, *digest)
        .unwrap();
    let intent = fence
        .confirm_review(
            &challenge,
            digest,
            &stopped,
            stopped.evidence(),
            stopped.profile_digest(),
        )
        .unwrap();
    let proof = "z".repeat(43);
    let proof_hash: [u8; 32] = Sha256::digest(proof.as_bytes()).into();
    let intent_digest = intent.proof_digest();
    // Exact-intent test proof; only the outer test uses the disposable tor_ci.
    sqlx::query(
        "UPDATE sessions SET step_up_proof_hash=$1,step_up_scope=$2,
         step_up_resource_id=1,step_up_revision=$3,step_up_payload_digest=$4,
         step_up_issued_at=clock_timestamp(),step_up_expires_at=clock_timestamp()+interval '2 minutes',
         step_up_used_at=NULL WHERE token_hash=$5",
    )
    .bind(proof_hash.as_slice())
    .bind(tor_contracts::auth::MAIL_ACTIVATION_STEP_UP_SCOPE)
    .bind(intent.policy_revision())
    .bind(intent_digest.as_slice())
    .bind(digest.as_slice())
    .execute(pool)
    .await
    .unwrap();
    launch
        .admin_activate(digest, &proof, &intent)
        .await
        .unwrap();
    let active = launch.snapshot().await.unwrap().unwrap();
    fence.complete_activation(intent, &active).unwrap();
    active
}

pub(super) async fn exercise_dispatch(pool: &PgPool, digest: &[u8; 32], actor: i64) {
    let queue = PgMailQueue::new(pool.clone());
    let quota = PgMailQuota::new(pool.clone());
    let feedback = PgPostboxFeedback::new(pool.clone());
    let launch = PgMailLaunch::new(pool.clone());
    let fence = LaunchFence::new();
    // Clear only the disposable rolling ledger, preserving the reconciled counter.
    clear(pool).await;
    let active = activate_dispatch(pool, digest, &fence).await;
    let attempt = claim(
        &queue,
        UserId::new(actor).unwrap(),
        2,
        MailPurpose::SupportReply,
    )
    .await;
    let key = actor_keys(pool, actor).await;
    let wrong_key = keys(250, None);
    fence
        .permit_reservation(&active, LaunchScope::Dispatch)
        .unwrap();
    assert_eq!(
        quota
            .reserve_for_dispatch(attempt, key, &active)
            .await
            .unwrap(),
        QuotaDecision::Reserved
    );
    let reserved = launch.snapshot().await.unwrap().unwrap();
    assert_eq!(
        quota
            .reserve_for_dispatch(attempt, key, &active)
            .await
            .unwrap(),
        QuotaDecision::CostEmergencyStop
    );
    assert_eq!(
        quota
            .reserve_for_dispatch(attempt, key, &reserved)
            .await
            .unwrap(),
        QuotaDecision::AlreadyReserved
    );
    assert_eq!(
        queue
            .defer_unsubmitted(attempt, DeferReason::ProviderUnavailable)
            .await
            .unwrap(),
        DeferOutcome::Refused
    );
    assert!(feedback
        .prepare(attempt, "ci-identity")
        .await
        .unwrap()
        .is_none());
    for (identity, candidate, snapshot) in [
        ("other-identity", key, &reserved),
        ("ci-identity", wrong_key, &reserved),
        ("ci-identity", key, &active),
    ] {
        assert!(feedback
            .prepare_for_dispatch(attempt, identity, candidate, snapshot)
            .await
            .unwrap()
            .is_none());
    }
    assert!(feedback
        .prepare_for_dispatch(attempt, "ci-identity", key, &reserved)
        .await
        .unwrap()
        .is_some());
    assert!(feedback
        .prepare_for_dispatch(attempt, "ci-identity", key, &reserved)
        .await
        .unwrap()
        .is_none());
    let current = launch.snapshot().await.unwrap().unwrap();
    assert_eq!(
        LaunchFence::new().consume_start(&current, LaunchScope::Dispatch),
        Err(LaunchError::Stopped)
    );
    fence
        .consume_start(&current, LaunchScope::Dispatch)
        .unwrap();
    assert_eq!(
        fence.consume_start(&current, LaunchScope::Dispatch),
        Err(LaunchError::Conflict)
    );
    // No network access or SMTP calls. Keep the reservation counted until
    // the isolated fixture cleanup below.
    assert!(queue.finish(attempt, MailOutcome::Cancelled).await.unwrap());
    PgMailCost::new(pool.clone())
        .admin_stop(digest)
        .await
        .unwrap();
    sqlx::query("DELETE FROM mail_feedback_attempts WHERE outbox_id=$1")
        .bind(attempt.id)
        .execute(pool)
        .await
        .unwrap();
    clear(pool).await;
}
