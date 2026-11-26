use sqlx::PgPool;
use std::time::{SystemTime, UNIX_EPOCH};
use tor_contracts::{
    auth::UserId,
    mail::{MailIntent, MailPurpose, RegistrationMailEvidence},
    paid_delivery::PositiveId,
};
use tor_mail::{
    domain::{
        quota::{identity::QuotaKeySigner, QuotaDecision, QuotaKeys},
        MailClaim, MailOutcome,
    },
    repositories::{quota::PgMailQuota, PgMailQueue},
};

fn keys(recipient: u8, ip: Option<u8>) -> QuotaKeys {
    QuotaKeySigner::from_hex(&"4f".repeat(32))
        .unwrap()
        .keys(
            &format!("mail-quota-fixture-{recipient}@example.invalid"),
            ip.map(|value| std::net::Ipv4Addr::new(192, 0, 2, value).into()),
        )
        .unwrap()
}

fn registration_keys(claim: MailClaim, recipient: u8, ip: Option<u8>, secret: &str) -> QuotaKeys {
    let signer = QuotaKeySigner::from_hex(&secret.repeat(32)).unwrap();
    let address = format!("mail-quota-fixture-{recipient}@example.invalid");
    let captured = signer
        .keys(
            &address,
            ip.map(|value| std::net::Ipv4Addr::new(192, 0, 2, value).into()),
        )
        .unwrap();
    signer
        .keys_for_registration(
            claim.intent,
            &address,
            RegistrationMailEvidence {
                user_id: claim.intent.recipient,
                source: captured.signup_source(),
            },
        )
        .unwrap()
}

async fn claim(queue: &PgMailQueue, user: UserId, event: i64, purpose: MailPurpose) -> MailClaim {
    assert!(queue
        .enqueue(MailIntent {
            purpose,
            event_id: PositiveId::new(event).unwrap(),
            recipient: user,
        })
        .await
        .unwrap());
    queue.claim_next().await.unwrap().unwrap()
}

async fn clear(pool: &PgPool) {
    sqlx::query("DELETE FROM mail_send_reservations")
        .execute(pool)
        .await
        .unwrap();
}

// Seed controlled ledger history without waiting 24h or sending mail.
async fn seed(pool: &PgPool, count: i64, purpose: MailPurpose, keys: QuotaKeys, age_seconds: i64) {
    sqlx::query(
        "INSERT INTO mail_send_reservations \
         (outbox_id,generation,purpose,recipient_key,registration_ip_key,reserved_at) \
         SELECT 900000000000+n,1,$2,$3,$4,clock_timestamp()-($5::bigint*interval '1 second') \
         FROM generate_series(1::bigint,$1::bigint) n",
    )
    .bind(count)
    .bind(purpose.as_str())
    .bind(keys.recipient().as_slice())
    .bind(keys.registration_ip().map(|key| key.as_slice()))
    .bind(age_seconds)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
#[ignore = "requires migrated disposable local tor_ci; never use production"]
async fn mail_01_f1_atomic_rolling_quota_reserve_and_restart() {
    use MailPurpose::*;
    use QuotaDecision::*;

    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let options: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
    assert_eq!(options.get_host(), "127.0.0.1");
    assert_eq!(options.get_database(), Some("tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    let queue = PgMailQueue::new(pool.clone());
    let quota = PgMailQuota::new(pool.clone());
    clear(&pool).await;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash) \
         VALUES($1,'Mail quota fixture','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("mail-quota-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let user = UserId::new(user).unwrap();

    let a = claim(&queue, user, 1, RegistrationConfirmation).await;
    let b = claim(&queue, user, 2, RegistrationConfirmation).await;
    assert_eq!(
        quota.reserve(a, keys(1, Some(1))).await.unwrap(),
        RegistrationEvidenceRequired
    );
    assert_eq!(
        quota
            .reserve(a, registration_keys(b, 2, Some(2), "4f"))
            .await
            .unwrap(),
        RegistrationEvidenceRequired
    );
    let (ra, rb) = tokio::join!(
        quota.reserve(a, registration_keys(a, 1, Some(1), "4f")),
        quota.reserve(b, registration_keys(b, 2, Some(2), "4f"))
    );
    let (ra, rb) = (ra.unwrap(), rb.unwrap());
    assert!(matches!(
        (ra, rb),
        (Reserved, GlobalPace) | (GlobalPace, Reserved)
    ));
    let (winner, winner_keys) = if ra == Reserved {
        (a, registration_keys(a, 1, Some(1), "4f"))
    } else {
        (b, registration_keys(b, 2, Some(2), "4f"))
    };
    let restarted = PgMailQuota::new(PgPool::connect(&url).await.unwrap());
    assert_eq!(
        restarted.reserve(winner, winner_keys).await.unwrap(),
        AlreadyReserved
    );
    let wrong_key = registration_keys(winner, 1, None, "5a");
    assert_eq!(quota.reserve(winner, wrong_key).await.unwrap(), KeyMismatch);
    assert_eq!(
        restarted.reserve(winner, winner_keys).await.unwrap(),
        AlreadyReserved
    );
    let mut stale = winner;
    stale.generation += 1;
    assert_eq!(quota.reserve(stale, winner_keys).await.unwrap(), StaleClaim);
    assert!(queue.finish(winner, MailOutcome::Uncertain).await.unwrap());
    let ledger: i64 = sqlx::query_scalar("SELECT count(*) FROM mail_send_reservations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(ledger, 1);

    let registration = claim(&queue, user, 3, RegistrationConfirmation).await;
    let news = claim(&queue, user, 4, ProjectNews).await;
    let support = claim(&queue, user, 5, SupportReply).await;

    clear(&pool).await;
    seed(&pool, 3, RegistrationConfirmation, keys(3, Some(3)), 180).await;
    assert_eq!(
        quota
            .reserve(registration, registration_keys(registration, 3, Some(4), "4f"))
            .await
            .unwrap(),
        RegistrationRecipientDaily
    );
    clear(&pool).await;
    seed(&pool, 1, RegistrationConfirmation, keys(3, Some(3)), 60).await;
    assert_eq!(
        quota
            .reserve(registration, registration_keys(registration, 3, Some(4), "4f"))
            .await
            .unwrap(),
        RegistrationRecipientCooldown
    );
    clear(&pool).await;
    seed(&pool, 10, RegistrationConfirmation, keys(4, Some(3)), 180).await;
    assert_eq!(
        quota
            .reserve(registration, registration_keys(registration, 3, Some(3), "4f"))
            .await
            .unwrap(),
        RegistrationIpDaily
    );
    clear(&pool).await;
    seed(&pool, 10, RegistrationConfirmation, keys(4, None), 180).await;
    assert_eq!(
        quota
            .reserve(registration, registration_keys(registration, 3, None, "4f"))
            .await
            .unwrap(),
        RegistrationIpDaily
    );

    clear(&pool).await;
    seed(&pool, 150, ProjectNews, keys(7, None), 180).await;
    assert_eq!(
        quota.reserve(news, keys(7, None)).await.unwrap(),
        TransactionalReserve
    );
    assert_eq!(
        quota.reserve(support, keys(7, None)).await.unwrap(),
        Reserved
    );

    clear(&pool).await;
    seed(&pool, 200, ProjectNews, keys(7, None), 180).await;
    assert_eq!(
        quota
            .reserve(registration, registration_keys(registration, 3, Some(3), "4f"))
            .await
            .unwrap(),
        GlobalDailyLimit
    );
    sqlx::query(
        "UPDATE mail_send_reservations SET reserved_at=clock_timestamp()-interval '25 hours'",
    )
    .execute(&pool)
    .await
    .unwrap();
    assert_eq!(
        quota
            .reserve(registration, registration_keys(registration, 3, Some(3), "4f"))
            .await
            .unwrap(),
        Reserved
    );

    // Removing the account must not refund the recent provider attempt.
    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user.get())
        .execute(&pool)
        .await
        .unwrap();
    let recent: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM mail_send_reservations WHERE reserved_at>clock_timestamp()-interval '24 hours'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(recent, 1);
    clear(&pool).await;
}

mod suppression;

mod postbox_feedback;
