use super::*;
use tor_mail::repositories::{cost::PgMailCost, postbox_feedback::PgPostboxFeedback};

async fn set_count(pool: &PgPool, value: i64) {
    sqlx::query("UPDATE mail_cost_months SET reserved_attempts=$1")
        .bind(value)
        .execute(pool)
        .await
        .unwrap();
}

async fn count(store: &PgMailCost) -> i64 {
    store
        .snapshot()
        .await
        .unwrap()
        .unwrap()
        .usage
        .unwrap()
        .reserved_attempts
}

#[tokio::test]
#[ignore = "requires migrated disposable local tor_ci; never use production"]
async fn mail_01_h1_monthly_envelope_is_atomic_durable_and_not_a_free_tier_cutoff() {
    use MailPurpose::*;
    use QuotaDecision::*;

    let url = std::env::var("TEST_DATABASE_URL").unwrap();
    let options: sqlx::postgres::PgConnectOptions = url.parse().unwrap();
    assert_eq!(options.get_host(), "127.0.0.1");
    assert_eq!(options.get_database(), Some("tor_ci"));
    let pool = PgPool::connect(&url).await.unwrap();
    clear(&pool).await;
    configure_cost_fixture(&pool).await;
    let quota = PgMailQuota::new(pool.clone());
    let queue = PgMailQueue::new(pool.clone());
    let cost = PgMailCost::new(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let user: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash) \
         VALUES($1,'Mail cost fixture','$argon2id$fixture') RETURNING id",
    )
    .bind(format!("mail-h1-{nonce}@example.invalid"))
    .fetch_one(&pool)
    .await
    .unwrap();
    let user = UserId::new(user).unwrap();
    let registration = claim(&queue, user, 1, RegistrationConfirmation).await;
    let key = registration_keys(registration, 11, Some(11), "4f");

    sqlx::query("UPDATE mail_cost_policy SET enabled=false")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        quota.reserve(registration, key).await.unwrap(),
        CostEmergencyStop
    );
    assert_eq!(count(&cost).await, 0);
    sqlx::query("UPDATE mail_cost_policy SET enabled=true")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM mail_cost_months")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        quota.reserve(registration, key).await.unwrap(),
        CostStateUnavailable
    );
    configure_cost_fixture(&pool).await;
    set_count(&pool, 2000).await;
    assert_eq!(quota.reserve(registration, key).await.unwrap(), Reserved);
    assert_eq!(count(&cost).await, 2001);
    assert_eq!(
        quota.reserve(registration, key).await.unwrap(),
        AlreadyReserved
    );
    assert_eq!(count(&cost).await, 2001);
    let feedback = PgPostboxFeedback::new(pool.clone());
    sqlx::query("UPDATE mail_cost_policy SET enabled=false")
        .execute(&pool)
        .await
        .unwrap();
    assert!(feedback
        .prepare(registration, "cost-identity-fixture")
        .await
        .unwrap()
        .is_none());
    assert_eq!(count(&cost).await, 2001);
    sqlx::query("UPDATE mail_cost_policy SET enabled=true")
        .execute(&pool)
        .await
        .unwrap();
    assert!(queue
        .finish(registration, MailOutcome::Uncertain)
        .await
        .unwrap());

    // Short quota-ledger cleanup cannot refund the monthly charge.
    clear(&pool).await;
    let restarted = PgMailCost::new(PgPool::connect(&url).await.unwrap());
    assert_eq!(count(&restarted).await, 2001);
    set_count(&pool, 6979).await;
    let news = claim(&queue, user, 2, ProjectNews).await;
    assert_eq!(
        quota.reserve(news, keys(12, None)).await.unwrap(),
        CostNewsPaused
    );
    assert_eq!(count(&cost).await, 6979);
    let support = claim(&queue, user, 3, SupportReply).await;
    assert_eq!(
        quota.reserve(support, keys(12, None)).await.unwrap(),
        Reserved
    );
    assert_eq!(count(&cost).await, 6980);

    clear(&pool).await;
    set_count(&pool, 8224).await;
    let first = claim(&queue, user, 4, SupportReply).await;
    let second = claim(&queue, user, 5, SupportReply).await;
    let (a, b) = tokio::join!(
        quota.reserve(first, keys(13, None)),
        quota.reserve(second, keys(14, None))
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert!(matches!(
        (a, b),
        (Reserved, GlobalPace | CostMonthlyLimit) | (GlobalPace | CostMonthlyLimit, Reserved)
    ));
    assert_eq!(count(&cost).await, 8225);
    let denied = if a == Reserved { second } else { first };
    clear(&pool).await;
    assert_eq!(
        quota.reserve(denied, keys(15, None)).await.unwrap(),
        CostMonthlyLimit
    );
    assert_eq!(count(&cost).await, 8225);
    let ledger: i64 = sqlx::query_scalar("SELECT count(*) FROM mail_send_reservations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(ledger, 0, "denied reservations must roll back");

    sqlx::query("DELETE FROM users WHERE id=$1")
        .bind(user.get())
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(count(&restarted).await, 8225);
    sqlx::query("UPDATE mail_cost_policy SET enabled=false")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM mail_cost_months")
        .execute(&pool)
        .await
        .unwrap();
}
