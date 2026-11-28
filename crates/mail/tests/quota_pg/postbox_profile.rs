use super::*;
use sha2::{Digest, Sha256};
use tor_contracts::auth::POSTBOX_PROFILE_STEP_UP_SCOPE;
use tor_mail::{
    domain::postbox_profile::{ProfileIdentity, ProfileMasterKey, ProfileSecrets},
    repositories::postbox_profile::{PgPostboxProfile, ProfileError},
};

#[tokio::test]
#[ignore = "requires migrated disposable local tor_ci; never use production"]
async fn mail_01_profile_sealed_candidate_requires_live_role_proof_and_revision() {
    use base64::{engine::general_purpose::STANDARD, Engine};
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let options: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
    assert_eq!(options.get_host(), "127.0.0.1");
    assert_eq!(options.get_database(), Some("tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let store = PgPostboxProfile::new(pool.clone());
    sqlx::query("UPDATE mail_cost_policy SET enabled=false WHERE singleton")
        .execute(&pool)
        .await
        .unwrap();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Postbox profile fixture','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("postbox-profile-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let digest: [u8; 32] = Sha256::digest(nonce.to_be_bytes()).into();
    sqlx::query(
        "INSERT INTO sessions(token_hash,user_id,expires_at)
         VALUES($1,$2,clock_timestamp()+interval '1 day')",
    )
    .bind(digest.as_slice())
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert!(matches!(
        store.admin_view(&digest).await,
        Err(ProfileError::Forbidden)
    ));
    sqlx::query(
        "INSERT INTO account_roles(user_id,role,is_active) VALUES($1,'administrator',true)",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    let revision = store.admin_view(&digest).await.unwrap().revision;
    let master = ProfileMasterKey::from_base64(1, &STANDARD.encode([7; 32])).unwrap();
    let secrets =
        ProfileSecrets::new("ci-profile-access".into(), "ci-profile-secret".into()).unwrap();
    let identity =
        ProfileIdentity::new("ci-identity".into(), "sender@example.invalid".into()).unwrap();
    let candidate = master
        .seal(identity.clone(), revision + 1, &secrets)
        .unwrap();
    let other = master.seal(identity, revision + 1, &secrets).unwrap();
    let proof = "x".repeat(43);
    assert_eq!(
        store
            .admin_save_candidate(&digest, &proof, revision, &candidate)
            .await,
        Err(ProfileError::StepUpRequired)
    );
    let proof_hash: [u8; 32] = Sha256::digest(proof.as_bytes()).into();
    sqlx::query(
        "UPDATE sessions SET step_up_proof_hash=$1,step_up_scope=$2,
         step_up_resource_id=1,step_up_revision=$3,step_up_payload_digest=$4,
         step_up_issued_at=clock_timestamp(),step_up_expires_at=clock_timestamp()+interval '2 minutes',
         step_up_used_at=NULL WHERE token_hash=$5",
    )
    .bind(proof_hash.as_slice())
    .bind(POSTBOX_PROFILE_STEP_UP_SCOPE)
    .bind(revision)
    .bind(candidate.intent_digest().as_slice())
    .bind(digest.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        store
            .admin_save_candidate(&digest, &proof, revision, &other)
            .await,
        Err(ProfileError::StepUpRequired)
    );
    sqlx::query("UPDATE mail_cost_policy SET enabled=true WHERE singleton")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        store
            .admin_save_candidate(&digest, &proof, revision, &candidate)
            .await,
        Err(ProfileError::Conflict)
    );
    sqlx::query("UPDATE mail_cost_policy SET enabled=false WHERE singleton")
        .execute(&pool)
        .await
        .unwrap();
    store
        .admin_save_candidate(&digest, &proof, revision, &candidate)
        .await
        .unwrap();
    let restarted = PgPostboxProfile::new(PgPool::connect(&url).await.unwrap());
    let view = restarted.admin_view(&digest).await.unwrap();
    assert_eq!(view.revision, revision + 1);
    assert_eq!(view.key_version, Some(1));
    let loaded = restarted
        .admin_load_candidate(&digest, revision + 1)
        .await
        .unwrap();
    let recovered = master.open(&loaded).unwrap();
    assert_eq!(recovered.access(), "ci-profile-access");
    assert_eq!(recovered.secret(), "ci-profile-secret");
    restarted
        .admin_confirm_probe(&digest, &loaded)
        .await
        .unwrap();
    assert_eq!(
        restarted.admin_confirm_probe(&digest, &other).await,
        Err(ProfileError::Conflict)
    );
    assert_eq!(
        restarted
            .admin_save_candidate(&digest, &proof, revision, &candidate)
            .await,
        Err(ProfileError::Conflict)
    );
    let used: bool =
        sqlx::query_scalar("SELECT step_up_used_at IS NOT NULL FROM sessions WHERE token_hash=$1")
            .bind(digest.as_slice())
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(used);
    let ciphertext: Vec<u8> =
        sqlx::query_scalar("SELECT ciphertext FROM mail_postbox_profile WHERE singleton")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(!String::from_utf8_lossy(&ciphertext).contains("ci-profile-secret"));
    let receipts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM mail_postbox_profile_audit WHERE actor_user_id=$1",
    )
    .bind(actor)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(receipts, 1);
    exercise_launch_fence(&pool, &digest, &loaded.intent_digest(), actor).await;
    sqlx::query("UPDATE account_roles SET is_active=false WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    assert!(matches!(
        restarted.admin_load_candidate(&digest, revision + 1).await,
        Err(ProfileError::Forbidden)
    ));
    // This is a disposable database; keep revision/audit monotonic.
    sqlx::query(
        "UPDATE mail_postbox_profile SET identity_id=NULL,sender=NULL,key_version=NULL,
         nonce=NULL,ciphertext=NULL,saved_at=NULL WHERE singleton",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("DELETE FROM account_roles WHERE user_id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(actor)
        .execute(&pool)
        .await
        .unwrap();
}

async fn exercise_launch_fence(
    pool: &PgPool,
    digest: &[u8; 32],
    profile_digest: &[u8; 32],
    actor: i64,
) {
    use tor_mail::{
        domain::cost::MonthUsage,
        repositories::{
            cost::{admin::MonthReconciliation, PgMailCost},
            launch::PgMailLaunch,
        },
        services::launch::{LaunchError, LaunchFence, LaunchScope},
    };

    let cost = PgMailCost::new(pool.clone());
    let launch = PgMailLaunch::new(pool.clone());
    // Disposable fixture only: create a genuinely reconciled, stopped month.
    // The production repository must never reset accounting or audit history.
    sqlx::query("DELETE FROM mail_cost_months")
        .execute(pool)
        .await
        .unwrap();
    assert!(launch.snapshot().await.unwrap().is_none());
    assert!(matches!(
        launch.admin_review_candidate(digest).await,
        Err(ProfileError::Conflict)
    ));
    let month: String = sqlx::query_scalar(
        "SELECT date_trunc('month',clock_timestamp() AT TIME ZONE 'Europe/Moscow')::date::text",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let known_attempts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM (
         SELECT outbox_id,generation FROM mail_send_reservations
         WHERE reserved_at>=($1::date::timestamp AT TIME ZONE 'Europe/Moscow')
         UNION SELECT outbox_id,generation FROM mail_feedback_attempts
         WHERE reserved_at>=($1::date::timestamp AT TIME ZONE 'Europe/Moscow')
         ) attempts",
    )
    .bind(&month)
    .fetch_one(pool)
    .await
    .unwrap();
    let revision = cost.admin_view(digest).await.unwrap().revision;
    let reconciliation = MonthReconciliation {
        usage: MonthUsage {
            month_start: month.clone(),
            free_allowance: 0,
            baseline_kopecks: 0,
            rate_per_thousand: 8032,
            reserved_attempts: known_attempts,
        },
        evidence_sha256: [17; 32],
    };
    cost.admin_reconcile_month(digest, revision, &reconciliation)
        .await
        .unwrap();
    let stopped = launch.snapshot().await.unwrap().unwrap();
    assert_eq!(stopped.policy_revision(), revision + 1);
    assert_eq!(stopped.month(), month);
    assert_eq!(stopped.profile_digest(), profile_digest);
    assert_eq!(stopped.evidence(), &reconciliation.evidence_sha256);
    let (review, candidate) = launch.admin_review_candidate(digest).await.unwrap();
    assert!(review == stopped);
    assert_eq!(&candidate.intent_digest(), profile_digest);
    assert!(matches!(
        launch.admin_review_candidate(&[0; 32]).await,
        Err(ProfileError::Forbidden)
    ));
    let scope = LaunchScope::OperatorTest(actor);
    let fence = LaunchFence::new();
    let challenge = fence.begin_review(&stopped, scope, *digest).unwrap();
    let intent = fence
        .confirm_review(
            &challenge,
            digest,
            &stopped,
            &reconciliation.evidence_sha256,
            profile_digest,
        )
        .unwrap();

    // The test uses a disposable, exact-intent password-proof fixture.
    // The actual web caller must obtain it from the fresh password verifier.
    let proof = "y".repeat(43);
    assert_eq!(
        launch.admin_activate(digest, &proof, &intent).await,
        Err(ProfileError::StepUpRequired)
    );
    let proof_hash: [u8; 32] = Sha256::digest(proof.as_bytes()).into();
    let intent_digest = intent.proof_digest();
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
    assert_eq!(
        launch.admin_activate(digest, &proof, &intent).await,
        Err(ProfileError::Conflict)
    );
    let active = launch.snapshot().await.unwrap().unwrap();
    assert!(matches!(
        launch.admin_review_candidate(digest).await,
        Err(ProfileError::Conflict)
    ));
    assert_eq!(
        fence.permit_reservation(&active, scope),
        Err(LaunchError::Stopped)
    );
    fence.complete_activation(intent, &active).unwrap();
    let receipt: (String, Option<i64>) = sqlx::query_as(
        "SELECT mode,test_user_id FROM mail_launch_activation_audit
         WHERE new_revision=$1",
    )
    .bind(active.policy_revision())
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(receipt, ("operator_test".to_owned(), Some(actor)));
    let queue = PgMailQueue::new(pool.clone());
    let probe_claim = claim(
        &queue,
        UserId::new(actor).unwrap(),
        901,
        MailPurpose::SupportReply,
    )
    .await;
    assert_eq!(
        PgMailQuota::new(pool.clone())
            .reserve(probe_claim, super::keys(19, None))
            .await
            .unwrap(),
        QuotaDecision::DispatchNotAuthorized
    );
    assert!(queue
        .finish(probe_claim, MailOutcome::Cancelled)
        .await
        .unwrap());
    assert_eq!(
        LaunchFence::new().permit_reservation(&active, scope),
        Err(LaunchError::Stopped)
    );
    fence.permit_reservation(&active, scope).unwrap();
    let grant = fence
        .authorize_operator_test(&active, actor, digest)
        .unwrap();
    let recipient: String = sqlx::query_scalar("SELECT email_normalized FROM users WHERE id=$1")
        .bind(actor)
        .fetch_one(pool)
        .await
        .unwrap();
    assert!(matches!(
        launch
            .operator_test_context(&grant, "foreign@example.invalid")
            .await,
        Err(ProfileError::Forbidden)
    ));
    let context = launch
        .operator_test_context(&grant, &recipient)
        .await
        .unwrap();
    assert_eq!(context.recipient(), recipient);
    assert_eq!(&context.profile().intent_digest(), profile_digest);
    assert!(launch.snapshot().await.unwrap().unwrap() == active);
    sqlx::query("UPDATE account_roles SET is_active=false WHERE user_id=$1")
        .bind(actor)
        .execute(pool)
        .await
        .unwrap();
    assert!(matches!(
        launch.operator_test_context(&grant, &recipient).await,
        Err(ProfileError::Forbidden)
    ));
    sqlx::query("UPDATE account_roles SET is_active=true WHERE user_id=$1")
        .bind(actor)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE mail_cost_months SET reserved_attempts=reserved_attempts+1 WHERE month_start=$1::date",
    )
    .bind(&month)
    .execute(pool)
    .await
    .unwrap();
    let reserved = launch.snapshot().await.unwrap().unwrap();
    assert!(matches!(
        launch.operator_test_context(&grant, &recipient).await,
        Err(ProfileError::Conflict)
    ));
    assert_eq!(
        fence.consume_start(&reserved, scope),
        Err(LaunchError::Conflict)
    );
    assert_eq!(
        fence.permit_reservation(&reserved, scope),
        Err(LaunchError::Stopped)
    );

    assert!(matches!(
        fence.authorize_dispatch(&reserved, probe_claim),
        Err(LaunchError::Stopped)
    ));
    cost.admin_stop(digest).await.unwrap();
    exercise_dispatch_grant(pool, digest, profile_digest, actor, reconciliation, &month).await;
    cost.admin_stop(digest).await.unwrap();
    sqlx::query("DELETE FROM mail_cost_months")
        .execute(pool)
        .await
        .unwrap();
    assert!(launch.snapshot().await.unwrap().is_none());
}

/// Use the same disposable operator/profile fixture and actual activation
/// transaction. No DNS, SMTP connection or real credentials.
async fn exercise_dispatch_grant(
    pool: &PgPool,
    session: &[u8; 32],
    profile_digest: &[u8; 32],
    actor: i64,
    mut reconciliation: tor_mail::repositories::cost::admin::MonthReconciliation,
    month: &str,
) {
    use tor_mail::{
        repositories::{
            cost::PgMailCost, launch::PgMailLaunch, postbox_feedback::PgPostboxFeedback,
        },
        services::launch::{LaunchError, LaunchFence, LaunchScope},
    };

    let cost = PgMailCost::new(pool.clone());
    let launch = PgMailLaunch::new(pool.clone());
    let quota = PgMailQuota::new(pool.clone());
    let feedback = PgPostboxFeedback::new(pool.clone());
    let queue = PgMailQueue::new(pool.clone());
    let fence = LaunchFence::new();
    let revision = cost.admin_view(session).await.unwrap().revision;
    reconciliation.usage.reserved_attempts = sqlx::query_scalar(
        "SELECT reserved_attempts FROM mail_cost_months WHERE month_start=$1::date",
    )
    .bind(month)
    .fetch_one(pool)
    .await
    .unwrap();
    reconciliation.evidence_sha256 = [19; 32];
    cost.admin_reconcile_month(session, revision, &reconciliation)
        .await
        .unwrap();
    let stopped = launch.snapshot().await.unwrap().unwrap();
    let challenge = fence
        .begin_review(&stopped, LaunchScope::Dispatch, *session)
        .unwrap();
    let intent = fence
        .confirm_review(
            &challenge,
            session,
            &stopped,
            &reconciliation.evidence_sha256,
            profile_digest,
        )
        .unwrap();
    let proof = "z".repeat(43);
    let proof_hash: [u8; 32] = Sha256::digest(proof.as_bytes()).into();
    let activation = intent.proof_digest();
    sqlx::query(
        "UPDATE sessions SET step_up_proof_hash=$1,step_up_scope=$2,
         step_up_resource_id=1,step_up_revision=$3,step_up_payload_digest=$4,
         step_up_issued_at=clock_timestamp(),step_up_expires_at=clock_timestamp()+interval '2 minutes',
         step_up_used_at=NULL WHERE token_hash=$5",
    )
    .bind(proof_hash.as_slice())
    .bind(tor_contracts::auth::MAIL_ACTIVATION_STEP_UP_SCOPE)
    .bind(intent.policy_revision())
    .bind(activation.as_slice())
    .bind(session.as_slice())
    .execute(pool)
    .await
    .unwrap();
    launch
        .admin_activate(session, &proof, &intent)
        .await
        .unwrap();
    let active = launch.snapshot().await.unwrap().unwrap();
    fence.complete_activation(intent, &active).unwrap();

    clear(pool).await;
    let user = UserId::new(actor).unwrap();
    let address: String = sqlx::query_scalar("SELECT email_normalized FROM users WHERE id=$1")
        .bind(actor)
        .fetch_one(pool)
        .await
        .unwrap();
    let signer = QuotaKeySigner::from_hex(&"4f".repeat(32)).unwrap();
    let (event, source) = super::reset_proof::reset_source(pool, user, &address, &signer).await;
    let reset = claim(&queue, user, event, MailPurpose::PasswordReset).await;
    assert_eq!(reset.intent.event_id.get(), event);
    assert_eq!(reset.intent.purpose, MailPurpose::PasswordReset);
    let other = claim(&queue, user, 101, MailPurpose::SupportReply).await;
    let keys = signer
        .keys_for_password_reset(
            reset.intent,
            &address,
            PasswordResetMailEvidence {
                intent: reset.intent,
                source,
            },
        )
        .unwrap();
    assert!(matches!(
        LaunchFence::new().authorize_dispatch(&active, reset),
        Err(LaunchError::Stopped)
    ));
    let grant = fence.authorize_dispatch(&active, reset).unwrap();
    let other_grant = fence.authorize_dispatch(&active, other).unwrap();
    let loaded = launch.dispatch_profile(&grant).await.unwrap().unwrap();
    assert_eq!(&loaded.intent_digest(), profile_digest);
    assert_eq!(
        quota.reserve(reset, keys).await.unwrap(),
        QuotaDecision::DispatchNotAuthorized
    );
    let reset_hash: [u8; 32] = Sha256::digest(activation).into();
    assert!(feedback
        .prepare_password_reset_dispatch(&grant, "ci-identity", &reset_hash, keys)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        quota.reserve_dispatch(&grant, keys).await.unwrap(),
        QuotaDecision::Reserved
    );
    let bound: Vec<u8> = sqlx::query_scalar(
        "SELECT launch_intent_digest FROM mail_send_reservations
         WHERE outbox_id=$1 AND generation=$2",
    )
    .bind(reset.id)
    .bind(reset.generation)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(bound.as_slice(), activation.as_slice());
    assert_eq!(
        quota.reserve_dispatch(&grant, keys).await.unwrap(),
        QuotaDecision::DispatchNotAuthorized
    );
    assert_eq!(
        quota
            .reserve_dispatch(&other_grant, super::keys(19, None))
            .await
            .unwrap(),
        QuotaDecision::DispatchNotAuthorized
    );
    assert!(feedback
        .prepare_dispatch(&other_grant, "ci-identity", super::keys(19, None))
        .await
        .unwrap()
        .is_none());
    assert!(feedback
        .prepare_password_reset(reset, "ci-identity", &reset_hash)
        .await
        .unwrap()
        .is_none());
    assert!(feedback
        .prepare_password_reset_dispatch(&grant, "wrong-identity", &reset_hash, keys)
        .await
        .unwrap()
        .is_none());
    let wrong = super::reset_keys(reset, 250, None);
    assert!(feedback
        .prepare_password_reset_dispatch(&grant, "ci-identity", &reset_hash, wrong)
        .await
        .unwrap()
        .is_none());
    let prepared = feedback
        .prepare_password_reset_dispatch(&grant, "ci-identity", &reset_hash, keys)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(prepared.claim(), reset);
    assert!(prepared.marker().starts_with("<tor-"));
    assert!(feedback
        .prepare_password_reset_dispatch(&grant, "ci-identity", &reset_hash, keys)
        .await
        .unwrap()
        .is_none());
    let start = launch.dispatch_start(prepared).await.unwrap().unwrap();
    fence.consume_dispatch_start(start).unwrap();
    // A simulated ambiguous submission, not a network call.
    assert!(feedback
        .finish_password_reset(
            reset,
            "ci-identity",
            &reset_hash,
            MailOutcome::Uncertain,
            None,
        )
        .await
        .unwrap());
    assert!(queue.finish(other, MailOutcome::Cancelled).await.unwrap());
    let current = launch.snapshot().await.unwrap().unwrap();
    assert_eq!(
        fence.consume_start(&current, LaunchScope::Dispatch),
        Err(LaunchError::Conflict)
    );
    clear(pool).await;
}
