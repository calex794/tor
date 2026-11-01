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
    // A source probing many targets is diagnostic only; no account is blocked.
    let spray_source = secret.source("203.0.113.4".parse().unwrap()).0;
    for index in 0..16 {
        let target = secret.target(&format!("unknown-{nonce}-{index}@example.invalid"));
        accounts
            .record_auth_observation(&AuthAttemptObservation {
                target_key: Some(target),
                user_id: None,
                source_key: Some(spray_source),
                subnet_key: None,
                agent: UserAgentFamily::Unknown,
                outcome: AuthAttemptOutcome::UnknownAccount,
            })
            .await
            .unwrap();
    }
    let signals = accounts.auth_risk_signals().await.unwrap();
    assert!(signals
        .iter()
        .any(|signal| signal.category == "source_spray"
            && signal.distinct_targets >= 16
            && signal.unknown_accounts >= 16));
    assert!(!signals
        .iter()
        .any(|signal| signal.category == "distributed_target"));
    // Distributed targeting: sixteen independent sources, one known target.
    let victim = secret.target(&format!("victim-{nonce}@example.invalid"));
    for index in 1..=16u8 {
        let source = secret.source(std::net::IpAddr::from([198, 18, 0, index])).0;
        accounts
            .record_auth_observation(&AuthAttemptObservation {
                target_key: Some(victim),
                user_id: Some(user_id),
                source_key: Some(source),
                subnet_key: None,
                agent: UserAgentFamily::Unknown,
                outcome: AuthAttemptOutcome::BadPassword,
            })
            .await
            .unwrap();
    }
    let signals = accounts.auth_risk_signals().await.unwrap();
    assert!(signals
        .iter()
        .any(|signal| signal.category == "distributed_target"
            && signal.distinct_sources >= 16
            && signal.bad_passwords >= 16));

    // A busy shared NAT with twenty successful logins is not a spray.
    let shared_source = secret.source("192.0.2.7".parse().unwrap()).0;
    for index in 0..20 {
        accounts
            .record_auth_observation(&AuthAttemptObservation {
                target_key: Some(secret.target(&format!("nat-{nonce}-{index}@example.invalid"))),
                user_id: None,
                source_key: Some(shared_source),
                subnet_key: None,
                agent: UserAgentFamily::Unknown,
                outcome: AuthAttemptOutcome::Success,
            })
            .await
            .unwrap();
    }
    let signals = accounts.auth_risk_signals().await.unwrap();
    assert!(!signals
        .iter()
        .any(|signal| signal.category == "source_spray" && signal.successes >= 20));

    // Ten typos by one person plus many successful neighbours are not
    // ten failed targets. Successful NAT traffic must not inflate the detector.
    for _ in 0..10 {
        accounts
            .record_auth_observation(&AuthAttemptObservation {
                target_key: Some(secret.target(&format!("nat-typos-{nonce}@example.invalid"))),
                user_id: None,
                source_key: Some(shared_source),
                subnet_key: None,
                agent: UserAgentFamily::Unknown,
                outcome: AuthAttemptOutcome::BadPassword,
            })
            .await
            .unwrap();
    }
    let signals = accounts.auth_risk_signals().await.unwrap();
    assert!(!signals
        .iter()
        .any(|signal| signal.category == "source_spray" && signal.successes >= 20));

    // Many successes plus a few failures must not reach the minimum 15
    // failed attempts, even if there are ten distinct mistyped targets.
    let busy_source = secret.source("192.0.2.99".parse().unwrap()).0;
    for index in 0..20 {
        accounts
            .record_auth_observation(&AuthAttemptObservation {
                target_key: Some(
                    secret.target(&format!("busy-good-{nonce}-{index}@example.invalid")),
                ),
                user_id: None,
                source_key: Some(busy_source),
                subnet_key: None,
                agent: UserAgentFamily::Unknown,
                outcome: AuthAttemptOutcome::Success,
            })
            .await
            .unwrap();
    }
    for index in 0..10 {
        accounts
            .record_auth_observation(&AuthAttemptObservation {
                target_key: Some(
                    secret.target(&format!("busy-typo-{nonce}-{index}@example.invalid")),
                ),
                user_id: None,
                source_key: Some(busy_source),
                subnet_key: None,
                agent: UserAgentFamily::Unknown,
                outcome: AuthAttemptOutcome::BadPassword,
            })
            .await
            .unwrap();
    }
    let signals = accounts.auth_risk_signals().await.unwrap();
    assert!(!signals
        .iter()
        .any(|signal| signal.category == "source_spray"
            && signal.bad_passwords == 10
            && signal.successes == 20));

    // Failed-source counts must exclude unrelated successful NAT users.
    // Twenty successes from another source against one attacked target must
    // not inflate the number of sources participating in the attack.
    for index in 1..=20u8 {
        let source = secret.source(std::net::IpAddr::from([203, 0, 113, index])).0;
        accounts
            .record_auth_observation(&AuthAttemptObservation {
                target_key: Some(victim),
                user_id: Some(user_id),
                source_key: Some(source),
                subnet_key: None,
                agent: UserAgentFamily::Unknown,
                outcome: AuthAttemptOutcome::Success,
            })
            .await
            .unwrap();
    }
    let signals = accounts.auth_risk_signals().await.unwrap();
    assert!(signals
        .iter()
        .any(|signal| signal.category == "distributed_target"
            && signal.bad_passwords == 16
            && signal.distinct_sources == 16
            && signal.successes == 20));

    // A flood with no trusted source must not be grouped into one fake IP.
    for index in 0..16 {
        accounts
            .record_auth_observation(&AuthAttemptObservation {
                target_key: Some(secret.target(&format!("anon-{nonce}-{index}@example.invalid"))),
                user_id: None,
                source_key: None,
                subnet_key: None,
                agent: UserAgentFamily::Unknown,
                outcome: AuthAttemptOutcome::UnknownAccount,
            })
            .await
            .unwrap();
    }
    let signals = accounts.auth_risk_signals().await.unwrap();
    assert!(!signals.iter().any(|signal| signal.key_hex.is_none()));

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

#[path = "auth_observation_pg/risk_load.rs"]
mod risk_load;
