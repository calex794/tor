//! Operator-only FND-07 scope and read-only checks before the first fixture write.
use super::super::support::{pg, Fixture};
use aws_sdk_s3::{
    config::{retry::RetryConfig, timeout::TimeoutConfig, BehaviorVersion, Credentials, Region},
    Client,
};
use std::{env, time::Duration};

pub(super) const YANDEX_ENDPOINT: &str = "https://storage.yandexcloud.net";
pub(super) const BUCKET_NOT_EMPTY: &str = "FND-07 bucket already contains objects";
const DISPOSABLE_DATABASE: &str = "postgresql://tor_app:tor-ci-app@127.0.0.1:5432/tor_test";

fn valid_scope(mode: &str, in_ci: bool, bucket: &str, database: &str) -> bool {
    mode == "READ_WRITE"
        && !in_ci
        && database == DISPOSABLE_DATABASE
        && bucket
            .strip_prefix("tor-fnd07-probe-")
            .is_some_and(|suffix| {
                (8..=40).contains(&suffix.len())
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            })
}

pub(super) async fn requested_fixture(label: &str) -> Option<(Fixture, String)> {
    let mode = match env::var("TOR_FND07_YANDEX_PROBE") {
        Ok(mode) => mode,
        Err(env::VarError::NotPresent) => return None,
        Err(_) => panic!("invalid provider opt-in"),
    };
    let bucket = env::var("TOR_FND07_YANDEX_BUCKET").expect("dedicated FND-07 bucket required");
    let url = env::var("TEST_DATABASE_URL").expect("isolated FND-07 PostgreSQL required");
    assert!(
        valid_scope(&mode, env::var_os("GITHUB_ACTIONS").is_some(), &bucket, &url),
        "FND-07 provider run requires READ_WRITE, a disposable local tor_test DB and a dedicated tor-fnd07-probe bucket outside GitHub Actions"
    );
    assert_eq!(
        pg(
            &url,
            "SELECT current_database()='tor_test' AND current_user='tor_app'
                   AND current_setting('server_version_num')::integer BETWEEN 180000 AND 189999
                   AND NOT pg_is_in_recovery() AND NOT EXISTS (SELECT 1 FROM books)
                   AND NOT EXISTS (SELECT 1 FROM chapter_import_jobs)"
        ),
        "t",
        "FND-07 requires a fresh PostgreSQL 18 catalog and empty queue"
    );

    let client = Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("ru-central1"))
            .credentials_provider(Credentials::new(
                env::var("TOR_FND07_YANDEX_ACCESS_KEY").expect("FND-07 access key required"),
                env::var("TOR_FND07_YANDEX_SECRET_KEY").expect("FND-07 secret key required"),
                None,
                None,
                "fnd07-isolated",
            ))
            .endpoint_url(YANDEX_ENDPOINT)
            .force_path_style(true)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .timeout_config(
                TimeoutConfig::builder()
                    .operation_timeout(Duration::from_secs(15))
                    .build(),
            )
            .build(),
    );
    check_bucket(&client, &bucket)
        .await
        .expect("FND-07 provider preflight");
    Some((
        Fixture::with_client(url, label, &bucket, client).await,
        bucket,
    ))
}

pub(super) async fn loopback_fixture(label: &str, bucket: &str) -> Fixture {
    // Match TOR's real non-versioned S3 target. Do not enable versioning,
    // even for disposable MinIO buckets: the live provider cannot undo it.
    let fixture = Fixture::new_in_bucket(label, bucket).await;
    check_bucket(&fixture.client, bucket)
        .await
        .expect("FND-07 loopback non-versioned preflight");
    fixture
}

pub(super) async fn check_bucket(client: &Client, bucket: &str) -> Result<(), &'static str> {
    let versioning = client
        .get_bucket_versioning()
        .bucket(bucket)
        .send()
        .await
        .map_err(|_| "could not read FND-07 bucket versioning")?;
    // A never-versioned bucket has no status; Enabled and Suspended
    // both fail, because historical versions might still exist.
    if versioning.status().is_some() {
        return Err("FND-07 bucket must be never-versioned");
    }
    let contents = client
        .list_objects_v2()
        .bucket(bucket)
        .max_keys(1)
        .send()
        .await
        .map_err(|_| "could not inspect FND-07 bucket objects")?;
    if contents.is_truncated().unwrap_or(false)
        || !contents.contents().is_empty()
        || !contents.common_prefixes().is_empty()
    {
        return Err(BUCKET_NOT_EMPTY);
    }
    Ok(())
}

#[test]
fn fnd07_provider_scope_rejects_shared_targets_and_implicit_execution() {
    const BUCKET: &str = "tor-fnd07-probe-01abc234";
    assert!(valid_scope(
        "READ_WRITE",
        false,
        BUCKET,
        DISPOSABLE_DATABASE
    ));
    assert!(!valid_scope("YES", false, BUCKET, DISPOSABLE_DATABASE));
    assert!(!valid_scope(
        "READ_WRITE",
        true,
        BUCKET,
        DISPOSABLE_DATABASE
    ));
    assert!(!valid_scope(
        "READ_WRITE",
        false,
        "tor-drop-probe-01abc234",
        DISPOSABLE_DATABASE
    ));
    assert!(!valid_scope(
        "READ_WRITE",
        false,
        "tor-fnd07-probe-01abc234.extra",
        DISPOSABLE_DATABASE
    ));
    assert!(!valid_scope(
        "READ_WRITE",
        false,
        BUCKET,
        "postgresql://tor_app@127.0.0.1:5432/tor"
    ));
}
