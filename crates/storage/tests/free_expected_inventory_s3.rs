//! DROP-07a: actual private MinIO GET/SHA evidence, never remote provider.
#![cfg(feature = "s3")]

use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    Client,
};
use sha2::{Digest, Sha256};
use tor_storage::{
    domain::provider::StorageError,
    repositories::free_expected_inventory_s3::{FreeObjectEvidence, S3FreeExpectedObjectVerifier},
};

#[tokio::test]
#[ignore = "requires disposable loopback MinIO, read-only verifier"]
async fn drop_07a_actual_bytes_are_verified_without_missing_or_gc_claims() {
    let endpoint = std::env::var("TOR_TEST_S3_ENDPOINT").expect("local MinIO");
    assert!(
        endpoint.starts_with("http://127.0.0.1:") || endpoint.starts_with("http://localhost:"),
        "never write to a remote provider"
    );
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            std::env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
            std::env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
            None,
            None,
            "drop07-loopback",
        ))
        .endpoint_url(endpoint)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    let client = Client::from_conf(config);
    const BUCKET: &str = "tor-private-drop07-ci";
    const KEY: &str = "private/free-chapters/70001/70001.md";
    client.create_bucket().bucket(BUCKET).send().await.unwrap();

    let actual = b"FREE immutable exact bytes".to_vec();
    let sha: [u8; 32] = Sha256::digest(&actual).into();
    client
        .put_object()
        .bucket(BUCKET)
        .key(KEY)
        .body(ByteStream::from(actual.clone()))
        .send()
        .await
        .unwrap();
    let verifier = S3FreeExpectedObjectVerifier::new(client.clone());
    assert_eq!(
        verifier
            .verify_exact(BUCKET, KEY, sha, actual.len() as u64)
            .await,
        Ok(FreeObjectEvidence::Verified)
    );
    let mut changed = actual.clone();
    changed[0] ^= 1;
    client
        .put_object()
        .bucket(BUCKET)
        .key(KEY)
        .body(ByteStream::from(changed))
        .send()
        .await
        .unwrap();
    assert_eq!(
        verifier
            .verify_exact(BUCKET, KEY, sha, actual.len() as u64)
            .await,
        Ok(FreeObjectEvidence::HashMismatch)
    );
    client
        .put_object()
        .bucket(BUCKET)
        .key(KEY)
        .body(ByteStream::from(b"short".to_vec()))
        .send()
        .await
        .unwrap();
    assert_eq!(
        verifier
            .verify_exact(BUCKET, KEY, sha, actual.len() as u64)
            .await,
        Ok(FreeObjectEvidence::SizeMismatch)
    );
    assert_eq!(
        verifier
            .verify_exact(
                BUCKET,
                "private/free-chapters/70001/70002.md",
                sha,
                actual.len() as u64
            )
            .await,
        Err(StorageError::Unavailable),
        "ambiguous GET error must never become a missing/orphan claim"
    );
    assert_eq!(
        verifier
            .verify_exact("public-media", KEY, sha, actual.len() as u64)
            .await,
        Err(StorageError::Invalid)
    );
    println!("DROP07A_MINIO_GET_SHA_PASS: intact, corrupt, short, unavailable and invalid all fail closed");
}
