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
    assert_eq!(prefs.delivery_preference(first).await.unwrap(), DeliveryMode::Disabled);
    assert_eq!(prefs.delivery_preference(second).await.unwrap(), DeliveryMode::Disabled);
    assert_eq!(prefs.delivery_preference(0).await, Err(PreferenceError::Invalid));
    assert_eq!(
        prefs.set_delivery_preference(0, DeliveryMode::Immediate).await,
        Err(PreferenceError::Invalid)
    );
    for mode in [DeliveryMode::Immediate, DeliveryMode::DailyDigest, DeliveryMode::Disabled] {
        prefs.set_delivery_preference(first, mode).await.unwrap();
        prefs.set_delivery_preference(first, mode).await.unwrap();
        assert_eq!(prefs.delivery_preference(first).await.unwrap(), mode);
        assert_eq!(prefs.delivery_preference(second).await.unwrap(), DeliveryMode::Disabled);
    }
    prefs.set_delivery_preference(second, DeliveryMode::DailyDigest).await.unwrap();
    assert_eq!(prefs.delivery_preference(first).await.unwrap(), DeliveryMode::Disabled);
    assert_eq!(prefs.delivery_preference(second).await.unwrap(), DeliveryMode::DailyDigest);
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
