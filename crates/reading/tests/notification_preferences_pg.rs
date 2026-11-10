//! C-17: persist delivery cadence privately without granting consent or access.
use sqlx::PgPool;
use tor_reading::repositories::{DeliveryMode, PgNotificationPreferences, PreferenceError};

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 tor_ci"]
async fn c_17_modes_default_disabled_and_are_actor_scoped() {
    let url = std::env::var("TEST_DATABASE_URL").expect("isolated postgres");
    assert!(url.ends_with("/tor_ci"), "production database forbidden");
    let pool = PgPool::connect(&url).await.expect("pool");
    let nonce: i64 =
        sqlx::query_scalar("SELECT (EXTRACT(EPOCH FROM clock_timestamp()) * 1000000)::bigint")
            .fetch_one(&pool)
            .await
            .unwrap();
    let mut users = Vec::new();
    for n in 1..=2 {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users(email_normalized,display_name,password_hash)
             VALUES($1,'C17 Reader','$argon2id$fixture') RETURNING id",
        )
        .bind(format!("c17-preference-{nonce}-{n}@example.invalid"))
        .fetch_one(&pool)
        .await
        .unwrap();
        users.push(id);
    }
    let first = users[0];
    let second = users[1];
    let prefs = PgNotificationPreferences::new(pool.clone());
    assert_eq!(
        prefs.delivery_preference(first).await.unwrap(),
        DeliveryMode::Disabled
    );
    assert_eq!(
        prefs.delivery_preference(second).await.unwrap(),
        DeliveryMode::Disabled
    );
    assert_eq!(
        prefs.delivery_preference(0).await,
        Err(PreferenceError::Invalid)
    );
    assert_eq!(
        prefs
            .set_delivery_preference(0, DeliveryMode::Immediate)
            .await,
        Err(PreferenceError::Invalid)
    );
    for mode in [
        DeliveryMode::Immediate,
        DeliveryMode::SmartTime,
        DeliveryMode::DailyDigest,
        DeliveryMode::Disabled,
    ] {
        prefs.set_delivery_preference(first, mode).await.unwrap();
        prefs.set_delivery_preference(first, mode).await.unwrap();
        assert_eq!(prefs.delivery_preference(first).await.unwrap(), mode);
        assert_eq!(
            prefs.delivery_preference(second).await.unwrap(),
            DeliveryMode::Disabled
        );
    }
    prefs
        .set_delivery_preference(second, DeliveryMode::DailyDigest)
        .await
        .unwrap();
    assert_eq!(
        prefs.delivery_preference(first).await.unwrap(),
        DeliveryMode::Disabled
    );
    assert_eq!(
        prefs.delivery_preference(second).await.unwrap(),
        DeliveryMode::DailyDigest
    );
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reader_notification_delivery_preferences WHERE user_id IN ($1,$2)",
    )
    .bind(first)
    .bind(second)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 2);
    pool.close().await;
}

#[tokio::test]
#[ignore = "requires isolated PostgreSQL 18 tor_ci"]
async fn s018_email_content_consent_requires_one_time_current_mailbox_proof() {
    use tor_reading::repositories::PgEmailNotificationConsent;

    let url = std::env::var("TEST_DATABASE_URL").expect("isolated postgres");
    assert!(url.ends_with("/tor_ci"), "never test against production");
    let pool = PgPool::connect(&url).await.expect("pool");
    let actor: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES(concat('s018-',gen_random_uuid()::text,'@example.invalid'),
                'S018 Proof Fixture','$argon2id$fixture') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let other: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES(concat('s018-',gen_random_uuid()::text,'@example.invalid'),
                'S018 Other Reader','$argon2id$fixture') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let settings = PgNotificationPreferences::new(pool.clone());
    let consent = PgEmailNotificationConsent::new(pool.clone());
    let proof_a = [17_u8; 32];
    let proof_b = [29_u8; 32];

    assert!(!consent.email_eligible(actor).await.unwrap());
    assert_eq!(consent.issue_proof(0, &proof_a).await, Err(PreferenceError::Invalid));
    assert!(consent.issue_proof(actor, &proof_a).await.unwrap());
    assert!(!consent.issue_proof(actor, &proof_b).await.unwrap(), "throttle");
    assert!(!consent.confirm_opt_in(other, &proof_a).await.unwrap());
    assert!(!consent.confirm_opt_in(actor, &proof_b).await.unwrap());
    assert!(consent.confirm_opt_in(actor, &proof_a).await.unwrap());
    assert!(!consent.confirm_opt_in(actor, &proof_a).await.unwrap(), "single-use");
    assert!(!consent.email_eligible(actor).await.unwrap(), "missing cadence stays off");

    settings
        .set_delivery_preference(actor, DeliveryMode::Immediate)
        .await
        .unwrap();
    assert!(consent.email_eligible(actor).await.unwrap());
    assert!(!consent.email_eligible(other).await.unwrap());

    settings
        .set_delivery_preference(actor, DeliveryMode::Disabled)
        .await
        .unwrap();
    assert!(!consent.email_eligible(actor).await.unwrap(), "explicit off wins");
    settings
        .set_delivery_preference(actor, DeliveryMode::SmartTime)
        .await
        .unwrap();
    assert!(consent.email_eligible(actor).await.unwrap());

    assert!(consent.revoke(actor).await.unwrap());
    assert!(!consent.email_eligible(actor).await.unwrap());
    assert!(!consent.confirm_opt_in(actor, &proof_a).await.unwrap());

    sqlx::query(
        "UPDATE reader_notification_email_proofs
         SET requested_at=statement_timestamp()-interval '65 seconds'
         WHERE user_id=$1",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert!(consent.issue_proof(actor, &proof_b).await.unwrap());
    // Changing the account email revokes the proof even if the old link arrives.
    sqlx::query(
        "UPDATE users SET email_normalized=concat('changed-',id::text,'@example.invalid')
         WHERE id=$1",
    )
    .bind(actor)
    .execute(&pool)
    .await
    .unwrap();
    assert!(!consent.confirm_opt_in(actor, &proof_b).await.unwrap());
    assert!(!consent.email_eligible(actor).await.unwrap());

    sqlx::query("DELETE FROM users WHERE id IN ($1,$2)")
        .bind(actor)
        .bind(other)
        .execute(&pool)
        .await
        .unwrap();
    let remains: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM reader_notification_email_proofs WHERE user_id IN ($1,$2)",
    )
    .bind(actor)
    .bind(other)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(remains, 0, "account erasure cascades to consent records");
    pool.close().await;
}
