//! The account proof must match one charged, correlated signup attempt.
use super::*;

async fn issue(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: MailClaim,
    revision: i64,
    marker: &str,
    digest: &[u8; 32],
) -> bool {
    sqlx::query_scalar("SELECT account_issue_registration_mail_proof($1,$2,$3,$4,$5,$6,$7)")
        .bind(claim.intent.recipient.get())
        .bind(claim.intent.event_id.get())
        .bind(claim.id)
        .bind(claim.generation)
        .bind(revision)
        .bind(marker)
        .bind(digest.as_slice())
        .fetch_one(&mut **tx)
        .await
        .unwrap()
}

async fn insert_marker(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    claim: MailClaim,
    marker: &str,
) {
    sqlx::query(
        "INSERT INTO mail_feedback_attempts
             (outbox_id,generation,recipient_key,key_binding,identity_id,marker,reserved_at)
         SELECT r.outbox_id,r.generation,r.recipient_key,g.key_binding,
                'registration-fixture',$3,r.reserved_at
         FROM mail_send_reservations r CROSS JOIN mail_send_budget_guard g
         WHERE r.outbox_id=$1 AND r.generation=$2 AND g.singleton",
    )
    .bind(claim.id)
    .bind(claim.generation)
    .bind(marker)
    .execute(&mut **tx)
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires migrated disposable local tor_ci; never use production"]
async fn mail_05_registration_proof_requires_exact_reserved_correlation() {
    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let options: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
    assert_eq!(options.get_host(), "127.0.0.1");
    assert_eq!(options.get_database(), Some("tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    configure_cost_fixture(&pool).await;
    clear(&pool).await;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let address = format!("mail-proof-{nonce}@example.invalid");
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Mail proof fixture','$argon2id$fixture') RETURNING id",
    )
    .bind(&address)
    .fetch_one(&pool)
    .await
    .unwrap();
    let user = UserId::new(id).unwrap();
    let signer = QuotaKeySigner::from_hex(&"4f".repeat(32)).unwrap();
    let source = signer.keys(&address, None).unwrap().signup_source();
    sqlx::query(
        "INSERT INTO account_registration_mail_sources
             (user_id,recipient_key,registration_ip_key,key_binding)
         VALUES($1,$2,NULL,$3)",
    )
    .bind(id)
    .bind(source.recipient_key.as_slice())
    .bind(source.key_binding.as_slice())
    .execute(&pool)
    .await
    .unwrap();

    let queue = PgMailQueue::new(pool.clone());
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
    let marker = format!("{nonce:064x}");
    let proof = [71; 32];
    let mut tx = pool.begin().await.unwrap();
    assert!(!issue(&mut tx, claim, 1, &marker, &proof).await);
    tx.rollback().await.unwrap();

    assert_eq!(
        PgMailQuota::new(pool.clone())
            .reserve(claim, keys)
            .await
            .unwrap(),
        QuotaDecision::Reserved
    );
    let mut tx = pool.begin().await.unwrap();
    assert!(!issue(&mut tx, claim, 1, &marker, &proof).await);
    insert_marker(&mut tx, claim, &marker).await;
    assert!(!issue(&mut tx, claim, 2, &marker, &proof).await);
    assert!(!issue(&mut tx, claim, 1, &"a".repeat(64), &proof).await);
    assert!(!issue(&mut tx, claim, 1, &marker, &[0; 32]).await);
    let mut stale = claim;
    stale.generation += 1;
    assert!(!issue(&mut tx, stale, 1, &marker, &proof).await);
    assert!(issue(&mut tx, claim, 1, &marker, &proof).await);
    assert!(!issue(&mut tx, claim, 1, &marker, &[72; 32]).await);
    tx.rollback().await.unwrap();

    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM account_email_ownership WHERE user_id=$1")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM mail_feedback_attempts WHERE outbox_id=$1")
            .bind(claim.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count, 0);

    // A lost handoff acknowledgement is harmless: the durable outbox is
    // authoritative even while forwarded_at remains NULL.
    // Reproduce the intended repository transaction with no network calls.
    let mut tx = pool.begin().await.unwrap();
    insert_marker(&mut tx, claim, &marker).await;
    assert!(issue(&mut tx, claim, 1, &marker, &proof).await);
    tx.commit().await.unwrap();
    let stored: (i64, i32, Vec<u8>) = sqlx::query_as(
        "SELECT outbox_id,generation,challenge_sha256
         FROM account_email_ownership WHERE user_id=$1",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(stored, (claim.id, claim.generation, proof.to_vec()));
    assert!(queue.finish(claim, MailOutcome::Cancelled).await.unwrap());
    let mut tx = pool.begin().await.unwrap();
    assert!(!issue(&mut tx, claim, 1, &marker, &[73; 32]).await);
    tx.rollback().await.unwrap();

    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM mail_feedback_attempts WHERE outbox_id=$1")
        .bind(claim.id)
        .execute(&pool)
        .await
        .unwrap();
    clear(&pool).await;
}
