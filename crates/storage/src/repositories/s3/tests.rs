//! A real loopback MinIO test for the G-only delete operation. This test
//! can construct a claim inside its own crate, not through public HTTP.
use super::*;
use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
};

#[tokio::test]
#[ignore = "requires loopback-only MinIO launched by storage-s3.yml"]
async fn g_s3_12_delete_claimed_private_object_and_repeat_safely() {
    let endpoint = std::env::var("TOR_TEST_S3_ENDPOINT").expect("MinIO URL");
    assert!(
        endpoint.starts_with("http://127.0.0.1:") || endpoint.starts_with("http://localhost:"),
        "never delete objects from a remote provider in CI"
    );
    let access = std::env::var("TOR_TEST_S3_ACCESS_KEY").expect("test key");
    let secret = std::env::var("TOR_TEST_S3_SECRET_KEY").expect("test secret");
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(access, secret, None, None, "gc-ci"))
        .endpoint_url(endpoint)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    let client = Client::from_conf(config);
    const BUCKET: &str = "tor-private-orphan-ci";
    const ID: &str = "a5d9cf20-8473-4108-a1f1-4b913c5d6ec5";
    client
        .create_bucket()
        .bucket(BUCKET)
        .send()
        .await
        .expect("disposable private bucket");
    let key = format!("private/chapter-versions/{ID}.enc");
    let ciphertext = vec![0x49_u8; 512];
    client
        .put_object()
        .bucket(BUCKET)
        .key(&key)
        .content_type(ENCRYPTED_CONTENT_TYPE)
        .body(ByteStream::from(ciphertext))
        .send()
        .await
        .unwrap();
    let claim = GcClaim::from_database(ID, "chapter", &key, BUCKET.to_owned(), 1, 1).unwrap();
    let store = S3PrivateStore::new(client.clone(), BUCKET.to_owned()).unwrap();
    let other_bucket = S3PrivateStore::new(client, "tor-private-other-ci".to_owned()).unwrap();
    assert_eq!(
        other_bucket.delete_claimed(&claim).await,
        Err(StorageError::Invalid),
        "bucket-swapped G lease cannot delete any object"
    );
    assert_eq!(store.delete_claimed(&claim).await, Ok(()));
    assert_eq!(
        store.delete_claimed(&claim).await,
        Ok(()),
        "DELETE after crash must be idempotent on missing S3 object"
    );
}
