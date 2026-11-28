//! Real common dispatch grant with a committed signup source. No network.
use super::*;
use tor_mail::{
    repositories::{launch::PgMailLaunch, postbox_feedback::PgPostboxFeedback},
    services::launch::LaunchFence,
};

pub(super) async fn exercise_registration_dispatch(pool: &PgPool, fence: &LaunchFence) {
    let queue = PgMailQueue::new(pool.clone());
    let quota = PgMailQuota::new(pool.clone());
    let launch = PgMailLaunch::new(pool.clone());
    let feedback = PgPostboxFeedback::new(pool.clone());
    let signer = QuotaKeySigner::from_hex(&"4f".repeat(32)).unwrap();

    // A successful admission, then a different attempt invalidated after
    // preparation. The latter must not consume a process send permit.
    for invalidate in [false, true] {
        clear(pool).await;
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let address = format!("registration-dispatch-{nonce}@example.invalid");
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'Registration dispatch fixture','$argon2id$fixture') RETURNING id",
        )
        .bind(&address)
        .fetch_one(pool)
        .await
        .unwrap();
        let user = UserId::new(id).unwrap();
        let source = signer.keys(&address, None).unwrap().signup_source();
        sqlx::query(
            "INSERT INTO account_registration_mail_sources
                 (user_id,recipient_key,registration_ip_key,key_binding)
             VALUES($1,$2,NULL,$3)",
        )
        .bind(id)
        .bind(source.recipient_key.as_slice())
        .bind(source.key_binding.as_slice())
        .execute(pool)
        .await
        .unwrap();
        let claim = claim(&queue, user, id, MailPurpose::RegistrationConfirmation).await;
        let keys = signer
            .keys_for_registration(
                claim.intent,
                &address,
                RegistrationMailEvidence {
                    user_id: user,
                    source,
                },
            )
            .unwrap();
        let snapshot = launch.snapshot().await.unwrap().unwrap();
        let grant = fence.authorize_dispatch(&snapshot, claim).unwrap();
        let proof = if invalidate { [83; 32] } else { [82; 32] };

        assert!(feedback
            .prepare_registration_dispatch(&grant, "ci-identity", &proof, 1, keys)
            .await
            .unwrap()
            .is_none());
        assert_eq!(
            quota.reserve_dispatch(&grant, keys).await.unwrap(),
            QuotaDecision::Reserved
        );
        assert!(feedback
            .prepare_dispatch(&grant, "ci-identity", keys)
            .await
            .unwrap()
            .is_none());
        assert!(feedback
            .prepare_registration_dispatch(&grant, "ci-identity", &proof, 2, keys)
            .await
            .unwrap()
            .is_none());
        let prepared = feedback
            .prepare_registration_dispatch(&grant, "ci-identity", &proof, 1, keys)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(prepared.claim(), claim);
        assert!(feedback
            .prepare_registration_dispatch(&grant, "ci-identity", &[84; 32], 1, keys)
            .await
            .unwrap()
            .is_none());
        if invalidate {
            sqlx::query("UPDATE users SET email_normalized=$2 WHERE id=$1")
                .bind(id)
                .bind(format!("registration-changed-{nonce}@example.invalid"))
                .execute(pool)
                .await
                .unwrap();
            assert!(launch.dispatch_start(prepared).await.unwrap().is_none());
        } else {
            let start = launch.dispatch_start(prepared).await.unwrap().unwrap();
            fence.consume_dispatch_start(start).unwrap();
        }
        // No real SMTP. Remove fixture proof/account after the simulated
        // terminal outcome; production finalization is still separate.
        assert!(queue.finish(claim, MailOutcome::Cancelled).await.unwrap());
        sqlx::query("DELETE FROM users WHERE id=$1")
            .bind(id)
            .execute(pool)
            .await
            .unwrap();
        sqlx::query("DELETE FROM mail_feedback_attempts WHERE outbox_id=$1")
            .bind(claim.id)
            .execute(pool)
            .await
            .unwrap();
    }
    clear(pool).await;
}
