use super::*;
use sha2::{Digest, Sha256};
use tor_mail::repositories::postbox_feedback::PgPostboxFeedback;

pub(super) async fn reset_source(
    pool: &PgPool,
    user: UserId,
    address: &str,
    signer: &QuotaKeySigner,
) -> (i64, tor_contracts::mail::PasswordResetMailSource) {
    let source = signer
        .keys(address, Some("192.0.2.7".parse().unwrap()))
        .unwrap()
        .password_reset_source();
    let event: i64 = sqlx::query_scalar(
        "INSERT INTO account_password_reset_mail_requests \
         (user_id,credential_fingerprint,recipient_key,request_ip_key,key_binding) \
         SELECT id,sha256(convert_to(password_hash,'UTF8')),$2,$3,$4 FROM users WHERE id=$1 \
         RETURNING id",
    )
    .bind(user.get())
    .bind(source.recipient_key.as_slice())
    .bind(source.request_ip_key.as_ref().map(|key| key.as_slice()))
    .bind(source.key_binding.as_slice())
    .fetch_one(pool)
    .await
    .unwrap();
    (event, source)
}

async fn live_proof(pool: &PgPool, proof: &[u8; 32]) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM password_reset_tokens WHERE token_hash=$1 \
         AND consumed_at IS NULL AND expires_at>clock_timestamp())",
    )
    .bind(proof.as_slice())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn correlation_exists(pool: &PgPool, claim: MailClaim) -> bool {
    sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM mail_feedback_attempts WHERE outbox_id=$1 AND generation=$2)",
    )
    .bind(claim.id)
    .bind(claim.generation)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "requires migrated disposable local tor_ci; never use production"]
async fn mail_01_reset_proof_atomic_preparation_and_account_revocation() {
    use tor_contracts::mail::PasswordResetMailEvidence;
    use MailPurpose::*;
    use QuotaDecision::*;

    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let options: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
    assert_eq!(options.get_host(), "127.0.0.1");
    assert_eq!(options.get_database(), Some("tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    configure_cost_fixture(&pool).await;
    clear(&pool).await;
    let queue = PgMailQueue::new(pool.clone());
    let quota = PgMailQuota::new(pool.clone());
    let feedback = PgPostboxFeedback::new(pool.clone());
    let signer = QuotaKeySigner::from_hex(&"4f".repeat(32)).unwrap();
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let address = format!("mail-reset-proof-{nonce}@example.invalid");
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash) \
         VALUES($1,'Reset proof fixture','$argon2id$fixture') RETURNING id",
    )
    .bind(&address)
    .fetch_one(&pool)
    .await
    .unwrap();
    let user = UserId::new(user).unwrap();
    let proof = |label: &str| -> [u8; 32] {
        Sha256::digest(format!("reset-proof-fixture-{nonce}-{label}").as_bytes()).into()
    };
    let old = proof("old");
    let first = proof("first");
    let second = proof("second");
    sqlx::query(
        "INSERT INTO password_reset_tokens(token_hash,user_id,credential_fingerprint) \
         SELECT $1,id,sha256(convert_to(password_hash,'UTF8')) FROM users WHERE id=$2",
    )
    .bind(old.as_slice())
    .bind(user.get())
    .execute(&pool)
    .await
    .unwrap();

    let (event, source) = reset_source(&pool, user, &address, &signer).await;
    let request = claim(&queue, user, event, PasswordReset).await;
    let keys = signer
        .keys_for_password_reset(
            request.intent,
            &address,
            PasswordResetMailEvidence {
                intent: request.intent,
                source,
            },
        )
        .unwrap();
    let identity = "reset-proof-fixture";
    assert!(feedback
        .prepare_password_reset(request, identity, &first)
        .await
        .unwrap()
        .is_none());
    assert!(!live_proof(&pool, &first).await);
    assert_eq!(quota.reserve(request, keys).await.unwrap(), Reserved);
    assert!(feedback.prepare(request, identity).await.unwrap().is_none());

    sqlx::query("UPDATE mail_cost_policy SET enabled=false")
        .execute(&pool)
        .await
        .unwrap();
    assert!(feedback
        .prepare_password_reset(request, identity, &first)
        .await
        .unwrap()
        .is_none());
    assert!(!correlation_exists(&pool, request).await);
    sqlx::query("UPDATE mail_cost_policy SET enabled=true")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO mail_recipient_suppression(recipient_key,reason,source_reserved_at) \
         VALUES($1,'complaint',clock_timestamp())",
    )
    .bind(source.recipient_key.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    assert!(feedback
        .prepare_password_reset(request, identity, &first)
        .await
        .unwrap()
        .is_none());
    assert!(!correlation_exists(&pool, request).await);
    sqlx::query("DELETE FROM mail_recipient_suppression WHERE recipient_key=$1")
        .bind(source.recipient_key.as_slice())
        .execute(&pool)
        .await
        .unwrap();

    let mut stale = request;
    stale.generation += 1;
    assert!(feedback
        .prepare_password_reset(stale, identity, &first)
        .await
        .unwrap()
        .is_none());
    let restarted = PgPostboxFeedback::new(PgPool::connect(&url).await.unwrap());
    let (a, b) = tokio::join!(
        feedback.prepare_password_reset(request, identity, &first),
        restarted.prepare_password_reset(request, identity, &second),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_ne!(a.is_some(), b.is_some());
    let (winner, loser) = if a.is_some() {
        (first, second)
    } else {
        (second, first)
    };
    assert!(correlation_exists(&pool, request).await);
    assert!(live_proof(&pool, &old).await);
    assert!(live_proof(&pool, &winner).await);
    assert!(!live_proof(&pool, &loser).await);
    let valid_duration: bool = sqlx::query_scalar(
        "SELECT expires_at-created_at=interval '30 minutes' FROM password_reset_tokens \
         WHERE token_hash=$1 AND password_reset_event_id=$2",
    )
    .bind(winner.as_slice())
    .bind(event)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(valid_duration);
    assert!(feedback
        .finish_password_reset(request, identity, &winner, MailOutcome::Uncertain, None)
        .await
        .unwrap());
    assert!(!feedback
        .finish_password_reset(request, identity, &winner, MailOutcome::Failed, None)
        .await
        .unwrap());
    assert!(live_proof(&pool, &winner).await);

    // A replacement source does not revoke a live proof, but an expired
    // or replaced request cannot issue another proof from a paid reservation.
    sqlx::query("DELETE FROM account_password_reset_mail_requests WHERE user_id=$1")
        .bind(user.get())
        .execute(&pool)
        .await
        .unwrap();
    assert!(live_proof(&pool, &winner).await);
    clear(&pool).await;
    let (event, source) = reset_source(&pool, user, &address, &signer).await;
    let request = claim(&queue, user, event, PasswordReset).await;
    let keys = signer
        .keys_for_password_reset(
            request.intent,
            &address,
            PasswordResetMailEvidence {
                intent: request.intent,
                source,
            },
        )
        .unwrap();
    assert_eq!(quota.reserve(request, keys).await.unwrap(), Reserved);
    sqlx::query(
        "UPDATE account_password_reset_mail_requests \
         SET created_at=clock_timestamp()-interval '11 minutes' WHERE id=$1",
    )
    .bind(event)
    .execute(&pool)
    .await
    .unwrap();
    assert!(feedback
        .prepare_password_reset(request, identity, &loser)
        .await
        .unwrap()
        .is_none());
    assert!(!correlation_exists(&pool, request).await);
    assert!(!live_proof(&pool, &loser).await);
    assert!(live_proof(&pool, &winner).await);

    // A definitive SMTP rejection revokes only the new proof. A stale
    // generation or unrelated digest cannot cancel an earlier live link.
    sqlx::query("DELETE FROM account_password_reset_mail_requests WHERE user_id=$1")
        .bind(user.get())
        .execute(&pool)
        .await
        .unwrap();
    assert!(queue.finish(request, MailOutcome::Cancelled).await.unwrap());
    clear(&pool).await;
    let (event, source) = reset_source(&pool, user, &address, &signer).await;
    let rejected = claim(&queue, user, event, PasswordReset).await;
    let keys = signer
        .keys_for_password_reset(
            rejected.intent,
            &address,
            PasswordResetMailEvidence {
                intent: rejected.intent,
                source,
            },
        )
        .unwrap();
    assert_eq!(quota.reserve(rejected, keys).await.unwrap(), Reserved);
    sqlx::query(
        "UPDATE mail_outbox SET lease_until=clock_timestamp()+interval '10 seconds' WHERE id=$1",
    )
    .bind(rejected.id)
    .execute(&pool)
    .await
    .unwrap();
    let rejected_proof = proof("definite-rejection");
    assert!(feedback
        .prepare_password_reset(rejected, identity, &rejected_proof)
        .await
        .unwrap()
        .is_none());
    assert!(!correlation_exists(&pool, rejected).await);
    sqlx::query(
        "UPDATE mail_outbox SET lease_until=clock_timestamp()+interval '120 seconds' WHERE id=$1",
    )
    .bind(rejected.id)
    .execute(&pool)
    .await
    .unwrap();
    assert!(feedback
        .prepare_password_reset(rejected, identity, &rejected_proof)
        .await
        .unwrap()
        .is_some());
    let mut stale = rejected;
    stale.generation += 1;
    assert!(!feedback
        .finish_password_reset(stale, identity, &rejected_proof, MailOutcome::Failed, None)
        .await
        .unwrap());
    assert!(!feedback
        .finish_password_reset(rejected, identity, &winner, MailOutcome::Failed, None)
        .await
        .unwrap());
    assert!(live_proof(&pool, &rejected_proof).await);
    let cost_before: i64 =
        sqlx::query_scalar("SELECT sum(reserved_attempts)::bigint FROM mail_cost_months")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(feedback
        .finish_password_reset(
            rejected,
            identity,
            &rejected_proof,
            MailOutcome::Failed,
            None,
        )
        .await
        .unwrap());
    assert!(!live_proof(&pool, &rejected_proof).await);
    assert!(live_proof(&pool, &old).await);
    assert!(live_proof(&pool, &winner).await);
    let cost_after: i64 =
        sqlx::query_scalar("SELECT sum(reserved_attempts)::bigint FROM mail_cost_months")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(cost_before, cost_after);

    // Accepted SMTP retains the issued proof and records the validated
    // provider identifier without treating a repeat as another send.
    sqlx::query("DELETE FROM account_password_reset_mail_requests WHERE user_id=$1")
        .bind(user.get())
        .execute(&pool)
        .await
        .unwrap();
    clear(&pool).await;
    let (event, source) = reset_source(&pool, user, &address, &signer).await;
    let accepted = claim(&queue, user, event, PasswordReset).await;
    let keys = signer
        .keys_for_password_reset(
            accepted.intent,
            &address,
            PasswordResetMailEvidence {
                intent: accepted.intent,
                source,
            },
        )
        .unwrap();
    assert_eq!(quota.reserve(accepted, keys).await.unwrap(), Reserved);
    let accepted_proof = proof("accepted");
    assert!(feedback
        .prepare_password_reset(accepted, identity, &accepted_proof)
        .await
        .unwrap()
        .is_some());
    let receipt = format!("receipt-{nonce}");
    assert!(feedback
        .finish_password_reset(
            accepted,
            identity,
            &accepted_proof,
            MailOutcome::Accepted,
            Some(&receipt),
        )
        .await
        .unwrap());
    assert!(live_proof(&pool, &accepted_proof).await);
    let stored: Option<String> = sqlx::query_scalar(
        "SELECT provider_message_id FROM mail_feedback_attempts WHERE outbox_id=$1 AND generation=$2",
    )
    .bind(accepted.id)
    .bind(accepted.generation)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored.as_deref(), Some(receipt.as_str()));
    assert!(!feedback
        .finish_password_reset(
            accepted,
            identity,
            &accepted_proof,
            MailOutcome::Cancelled,
            None,
        )
        .await
        .unwrap());

    sqlx::query("UPDATE users SET email_normalized=$2 WHERE id=$1")
        .bind(user.get())
        .bind(format!("changed-{address}"))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("UPDATE users SET email_normalized=$2 WHERE id=$1")
        .bind(user.get())
        .bind(&address)
        .execute(&pool)
        .await
        .unwrap();
    assert!(!live_proof(&pool, &old).await);
    assert!(!live_proof(&pool, &winner).await);
    assert!(!live_proof(&pool, &accepted_proof).await);
    assert!(feedback
        .prepare_password_reset(request, identity, &loser)
        .await
        .unwrap()
        .is_none());
    assert!(!correlation_exists(&pool, request).await);

    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user.get())
        .execute(&pool)
        .await
        .unwrap();
    clear(&pool).await;
}
