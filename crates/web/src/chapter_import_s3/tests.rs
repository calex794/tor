//! S023 private S3 staging provider tests against disposable loopback MinIO.
use super::S3ChapterImportStore;
use aws_sdk_s3::{
    config::{BehaviorVersion, Credentials, Region},
    Client,
};
use sha2::{Digest, Sha256};
use std::{process::Command, time::SystemTime};
use tor_books::services::chapter_import_jobs::{
    ChapterImportObjectStore, ChapterImportStoreError,
};

fn anonymous_status(url: &str) -> String {
    let output = Command::new("curl")
        .args(["--silent", "--output", "/dev/null", "--write-out", "%{http_code}", url])
        .output()
        .expect("curl installed in CI");
    assert!(output.status.success());
    String::from_utf8(output.stdout).unwrap()
}

#[tokio::test]
#[ignore = "only on disposable loopback MinIO from the chapter-import S3 workflow"]
async fn s023_private_source_put_get_conflict_and_delete() {
    let endpoint = std::env::var("TOR_TEST_S3_ENDPOINT").expect("MinIO URL");
    assert!(
        endpoint.starts_with("http://127.0.0.1:") || endpoint.starts_with("http://localhost:"),
        "must never run against production S3"
    );
    let access = std::env::var("TOR_TEST_S3_ACCESS_KEY").expect("test access key");
    let secret = std::env::var("TOR_TEST_S3_SECRET_KEY").expect("test secret key");
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(access, secret, None, None, "loopback-test"))
        .endpoint_url(&endpoint)
        .force_path_style(true)
        .build();
    let client = Client::from_conf(config);
    let nonce = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let bucket = format!("tor-chapter-import-{nonce}");
    client.create_bucket().bucket(&bucket).send().await.unwrap();
    let store = S3ChapterImportStore::new(client, bucket.clone()).unwrap();

    let key = format!("staging/chapters/42/99/{nonce}");
    let bytes = "Глава 1: Проверка\nСодержимое главы".as_bytes();
    let hash: [u8; 32] = Sha256::digest(bytes).into();
    assert_eq!(store.put_verified(&key, bytes, &hash).await, Ok(()));
    assert_eq!(
        store
            .get_verified(&key, bytes.len() as i64, &hash, 700_000)
            .await
            .unwrap(),
        bytes
    );
    // Retry after an unknown S3 PUT result is safe only for identical bytes.
    assert_eq!(store.put_verified(&key, bytes, &hash).await, Ok(()));

    let other = b"Different text for the same immutable object key";
    let other_hash: [u8; 32] = Sha256::digest(other).into();
    assert_eq!(
        store.put_verified(&key, other, &other_hash).await,
        Err(ChapterImportStoreError::Conflict)
    );
    assert_eq!(
        store
            .get_verified(&key, bytes.len() as i64, &hash, 700_000)
            .await
            .unwrap(),
        bytes,
        "conditional PUT must not overwrite the first object"
    );
    assert_eq!(
        anonymous_status(&format!("{endpoint}/{bucket}/{key}")),
        "403",
        "staging object must remain private"
    );

    store.delete(&key).await.unwrap();
    assert_eq!(
        store
            .get_verified(&key, bytes.len() as i64, &hash, 700_000)
            .await,
        Err(ChapterImportStoreError::Unavailable)
    );
}

#[test]
fn s3_import_store_rejects_unsafe_bucket_names() {
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            "unused",
            "unused",
            None,
            None,
            "unit",
        ))
        .build();
    for bucket in ["", "../private", "TOR_PRIVATE", "with space", "bad..bucket"] {
        assert!(S3ChapterImportStore::new(Client::from_conf(config.clone()), bucket.to_owned()).is_err());
    }
}
