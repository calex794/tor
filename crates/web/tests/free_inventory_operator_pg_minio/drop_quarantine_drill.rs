//! DROP-07i: exact-version retirement in an isolated S3 and PG18 drill.
#![cfg(feature = "s3-free")]

use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    types::{BucketVersioningStatus, VersioningConfiguration},
    Client,
};
use std::{
    env,
    io::Write,
    process::{Command, Stdio},
    time::{SystemTime, UNIX_EPOCH},
};
use tor_books::repositories::{
    free_key_quarantine::{FreeQuarantineError, PgFreeKeyQuarantine, QuarantineOutcome},
    PgCatalog,
};
use tor_storage::repositories::free_quarantine_s3::S3FreeQuarantineStore;
use tor_web::free_pack_worker::quarantine_operator::{
    enter_quarantine, recheck_quarantine, retire_drill_object, QuarantineOperatorError,
    VersionLocator,
};

fn local_s3() -> Client {
    let endpoint = env::var("TOR_TEST_S3_ENDPOINT").unwrap();
    let parsed = reqwest::Url::parse(&endpoint).unwrap();
    assert_eq!(parsed.scheme(), "http");
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert!(parsed.path() == "/" && parsed.query().is_none());
    Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new(
                env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
                env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
                None,
                None,
                "drop07i-drill",
            ))
            .endpoint_url(endpoint)
            .force_path_style(true)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .build(),
    )
}

async fn versioned(client: &Client, bucket: &str) {
    client.create_bucket().bucket(bucket).send().await.unwrap();
    client
        .put_bucket_versioning()
        .bucket(bucket)
        .versioning_configuration(
            VersioningConfiguration::builder()
                .status(BucketVersioningStatus::Enabled)
                .build(),
        )
        .send()
        .await
        .unwrap();
}

async fn put(client: &Client, bucket: &str, key: &str, bytes: &'static [u8]) -> String {
    let response = client
        .put_object()
        .bucket(bucket)
        .key(key)
        .body(ByteStream::from_static(bytes))
        .send()
        .await
        .unwrap();
    let version = response.version_id().expect("versioning enabled");
    assert_ne!(version, "null");
    version.to_owned()
}

#[tokio::test]
#[ignore = "requires disposable PG18/tor_ci and 127.0.0.1 MinIO"]
async fn drop_07i_preserves_newer_version_and_resumes_after_lost_delete_ack() {
    let database = env::var("TEST_DATABASE_URL").unwrap();
    let parsed = reqwest::Url::parse(&database).unwrap();
    assert_eq!(parsed.host_str(), Some("127.0.0.1"));
    assert_eq!(parsed.path(), "/tor_ci");
    assert!(parsed.query().is_none());
    let catalog = PgCatalog::connect(&database).await.unwrap();
    let db = PgFreeKeyQuarantine::new(catalog.pool().clone());
    let client = local_s3();
    let s3 = S3FreeQuarantineStore::new(client.clone());
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let main_bucket = format!("tor-gc-drill-primary-{stamp}");
    let backup_bucket = format!("tor-gc-drill-backup-{stamp}");
    versioned(&client, &main_bucket).await;
    versioned(&client, &backup_bucket).await;
    let key = "private/free-chapters/737777/168888.md";
    let untouched_key = "private/free-chapters/737777/168889.md";
    let old = put(&client, &main_bucket, key, b"old fixture bytes").await;
    let copy = put(&client, &backup_bucket, key, b"old fixture bytes").await;
    let wrong = put(&client, &backup_bucket, key, b"different bytes").await;
    let untouched = put(&client, &main_bucket, untouched_key, b"keep as-is").await;
    let locator = VersionLocator {
        bucket: &main_bucket,
        key,
        version: &old,
    };
    let backup = VersionLocator {
        bucket: &backup_bucket,
        key,
        version: &copy,
    };
    let unrelated = VersionLocator {
        bucket: &main_bucket,
        key: untouched_key,
        version: &untouched,
    };
    assert_eq!(
        enter_quarantine(&db, &s3, &unrelated).await,
        Ok(QuarantineOutcome::Recorded)
    );
    assert_eq!(
        recheck_quarantine(&db, &s3, &s3, &unrelated, &backup).await,
        Err(QuarantineOperatorError::EvidenceMismatch)
    );
    assert_eq!(
        retire_drill_object(&db, &s3, &s3, &unrelated).await,
        Err(QuarantineOperatorError::MissingVersion)
    );

    let original = s3
        .read_exact_version(&main_bucket, key, &old)
        .await
        .unwrap()
        .unwrap();
    let mut psql = Command::new("psql")
        .args([
            "-Xq",
            "-v",
            "ON_ERROR_STOP=1",
            "--dbname",
            &database,
            "-v",
            &format!("bucket={main_bucket}"),
            "-v",
            &format!("key={key}"),
            "-v",
            &format!("version={old}"),
            "-v",
            &format!("etag={}", original.etag),
            "-v",
            &format!("sha={}", hex::encode(original.sha256)),
            "-v",
            &format!("size={}", original.size),
        ])
        .stdin(Stdio::piped())
        .spawn()
        .unwrap();
    // Simulate elapsed retention; production writers cannot update a fence.
    psql.stdin
        .take()
        .unwrap()
        .write_all(
            b"INSERT INTO free_storage_key_fences
          VALUES (:'bucket',:'key','quarantine',:'version',:'etag',
                  decode(:'sha','hex'),:'size',clock_timestamp()-interval '25 hours');\n",
        )
        .unwrap();
    assert!(psql.wait().unwrap().success());
    assert_eq!(
        db.reserve_publication(&main_bucket, &[key]).await,
        Err(FreeQuarantineError::Protected)
    );
    assert_eq!(
        recheck_quarantine(
            &db,
            &s3,
            &s3,
            &locator,
            &VersionLocator {
                version: &wrong,
                ..backup
            }
        )
        .await,
        Err(QuarantineOperatorError::EvidenceMismatch)
    );
    assert_eq!(
        recheck_quarantine(&db, &s3, &s3, &locator, &backup).await,
        Ok(QuarantineOutcome::Recorded)
    );

    // A provider-scope write bypasses the app in this synthetic fixture. The
    // newer revision must still be present after an exact-version deletion.
    let newer = put(&client, &main_bucket, key, b"newer fixture version").await;
    let release = db
        .find_release(&main_bucket, key, &old)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        db.request_retirement(&release).await,
        Ok(QuarantineOutcome::Recorded)
    );
    s3.remove_verified_version(&main_bucket, key, &original)
        .await
        .unwrap();
    // Simulate cancellation before the success receipt was written.
    assert_eq!(
        retire_drill_object(&db, &s3, &s3, &locator).await,
        Ok(QuarantineOutcome::Recorded)
    );
    assert_eq!(
        retire_drill_object(&db, &s3, &s3, &locator).await,
        Ok(QuarantineOutcome::AlreadyRecorded)
    );
    assert!(s3
        .read_exact_version(&main_bucket, key, &old)
        .await
        .unwrap()
        .is_none());
    assert!(s3
        .read_exact_version(&main_bucket, key, &newer)
        .await
        .unwrap()
        .is_some());
    assert!(s3
        .read_exact_version(&main_bucket, untouched_key, &untouched)
        .await
        .unwrap()
        .is_some());
    assert!(s3
        .read_exact_version(&backup_bucket, key, &copy)
        .await
        .unwrap()
        .is_some());
}
