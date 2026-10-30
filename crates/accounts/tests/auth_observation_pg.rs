//! A-09: trusted pseudonymous observations and operator aggregates on isolated PG.
use std::time::{SystemTime, UNIX_EPOCH};
use tor_accounts::{
    domain::{
        access_audit::UserAgentFamily,
        auth_observation::{AuthAttemptObservation, AuthAttemptOutcome},
    },
    repositories::PgAccounts,
    services::auth_observation::AuthObservationSigner,
};

#[tokio::test]
#[ignore = "requires disposable PostgreSQL tor_ci"]
async fn a_09_observes_known_unknown_and_distinct_sources_without_raw_identity() {
    let url = std::env::var("TEST_DATABASE_URL").expect("test database");
    assert!(url.ends_with("/tor_ci"), "never touch production");
    let pool = sqlx::PgPool::connect(&url).await.unwrap();
    let accounts = PgAccounts::from_pool(pool.clone());
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let secret = AuthObservationSigner::from_hex(&"92".repeat(32)).unwrap();
    let first_email = format!("spray-{nonce}-a@example.invalid");
    let second_email = format!("spray-{nonce}-b@example.invalid");
    let first = secret.target(&first_email);
    let second = secret.target(&second_email);
    let (ip_a, net_a) = secret.source("198.51.100.11".parse().unwrap());
    let (ip_b, net_b) = secret.source("198.51.100.12".parse().unwrap());
    assert_eq!(net_a, net_b);
    let user_id: i64 = sqlx::query_scalar(
        "INSERT INTO users(email_normalized,display_name,password_hash)
         VALUES($1,'Spray Reader','$argon2id$fixture') RETURNING id",
    )
    .bind(&first_email)
    .fetch_one(&pool)
    .await
    .unwrap();

    for (target, source, result, user) in [
        (first, ip_a, AuthAttemptOutcome::BadPassword, Some(user_id)),
        (second, ip_a, AuthAttemptOutcome::UnknownAccount, None),
        (first, ip_b, AuthAttemptOutcome::Success, Some(user_id)),
    ] {
        accounts
            .record_auth_observation(&AuthAttemptObservation {
                target_key: Some(target),
                user_id: user,
                source_key: Some(source),
                subnet_key: Some(net_a),
                agent: UserAgentFamily::Unknown,
                outcome: result,
            })
            .await
            .unwrap();
    }
    let row: (i64, i64, i64) = sqlx::query_as(
        "SELECT count(*), count(DISTINCT target_key), count(DISTINCT source_key)
         FROM auth_login_observations WHERE target_key IN ($1,$2)",
    )
    .bind(first.as_slice())
    .bind(second.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row, (3, 2, 2));
    let summary: (i64, i64) = sqlx::query_as(
        "SELECT attempts, distinct_sources FROM auth_observation_targets_24h WHERE target_key=$1",
    )
    .bind(first.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(summary, (2, 2));
    let unknown: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_login_observations
         WHERE target_key=$1 AND outcome='unknown_account' AND user_id IS NULL",
    )
    .bind(second.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unknown, 1);
    // Raw values must never occur in rows, including JSON-like debug strings.
    let unsafe_values: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_login_observations WHERE
         target_key::text ILIKE '%example.invalid%' OR source_key::text ILIKE '%198.51.100%'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(unsafe_values, 0);
    sqlx::query(
        "UPDATE auth_login_observations SET observed_at=now()-INTERVAL '8 days'
                 WHERE target_key IN ($1,$2)",
    )
    .bind(first.as_slice())
    .bind(second.as_slice())
    .execute(&pool)
    .await
    .unwrap();
    accounts.prune_security_data().await.unwrap();
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM auth_login_observations WHERE target_key IN ($1,$2)",
    )
    .bind(first.as_slice())
    .bind(second.as_slice())
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(remaining, 0);
}
