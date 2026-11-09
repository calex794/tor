//! S349: real MinIO isolation for unencrypted immutable FREE chapter files.
#![cfg(feature = "s3")]

use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    Client,
};
use std::time::Duration;
use tor_storage::{
    domain::provider::StorageError,
    repositories::free_chapter_s3::S3FreeChapterStore,
    services::free_chapter::{
        upload_and_verify, FreeChapterObject, FreeChapterStore, FREE_MARKDOWN_CONTENT_TYPE,
    },
};

#[tokio::test]
#[ignore = "requires disposable loopback MinIO, never a remote provider"]
async fn s349_free_markdown_put_verify_replay_and_corruption_denial() {
    let endpoint = std::env::var("TOR_TEST_S3_ENDPOINT").expect("local MinIO");
    assert!(
        endpoint.starts_with("http://127.0.0.1:") || endpoint.starts_with("http://localhost:"),
        "never write to a production S3 provider"
    );
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            std::env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
            std::env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
            None,
            None,
            "s349-loopback",
        ))
        .endpoint_url(endpoint.clone())
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    let client = Client::from_conf(config);
    const BUCKET: &str = "tor-private-free-ci";
    client.create_bucket().bucket(BUCKET).send().await.unwrap();

    let text = "# Глава\n\nПроверяем 😀 中文";
    let object = FreeChapterObject::for_markdown(43001, 53001, text).unwrap();
    let store = S3FreeChapterStore::new(client.clone(), BUCKET.to_owned()).unwrap();
    assert_eq!(
        upload_and_verify(&store, &object, text)
            .await
            .unwrap()
            .object(),
        &object
    );
    assert_eq!(store.get_verified(&object).await.unwrap(), text.as_bytes());
    assert!(upload_and_verify(&store, &object, text).await.is_ok());

    let other_text = "# Изменённая версия";
    let other = FreeChapterObject::for_markdown(43001, 53001, other_text).unwrap();
    assert_eq!(
        store.put_immutable(&other, other_text.as_bytes()).await,
        Err(StorageError::Conflict),
        "same edition cannot be overwritten"
    );
    assert_eq!(
        upload_and_verify(&store, &other, other_text).await.err(),
        Some(StorageError::Corrupt),
        "replay with changed bytes cannot silently succeed"
    );
    assert_eq!(store.get_verified(&object).await.unwrap(), text.as_bytes());

    // Private S3 is inaccessible without the presigned URL.
    let anonymous = format!("{endpoint}/{BUCKET}/{}", object.key());
    let status = std::process::Command::new("curl")
        .args([
            "--silent",
            "--output",
            "/dev/null",
            "--write-out",
            "%{http_code}",
            &anonymous,
        ])
        .output()
        .unwrap();
    assert!(status.status.success());
    assert_eq!(String::from_utf8(status.stdout).unwrap(), "403");

    let signed = store
        .presign_verified(&object, Duration::from_secs(90))
        .await
        .unwrap();
    let url = serde_json::to_value(signed).unwrap();
    let url = url.as_str().unwrap();
    assert!(url.contains("X-Amz-Signature="));
    let fetched = std::process::Command::new("curl")
        .args(["--fail", "--silent", "--show-error", url])
        .output()
        .unwrap();
    assert!(fetched.status.success());
    assert_eq!(fetched.stdout, text.as_bytes());

    // Even a forged Content-Type and checksum metadata cannot conceal
    // wrong bytes when the B publisher requires GET verification.
    client
        .put_object()
        .bucket(BUCKET)
        .key(object.key())
        .content_type(FREE_MARKDOWN_CONTENT_TYPE)
        .metadata("sha256", hex::encode(object.sha256()))
        .body(ByteStream::from(b"tampered bytes".to_vec()))
        .send()
        .await
        .unwrap();
    assert_eq!(
        store.get_verified(&object).await,
        Err(StorageError::Corrupt)
    );
    assert_eq!(
        upload_and_verify(&store, &object, text).await.err(),
        Some(StorageError::Corrupt)
    );
}
