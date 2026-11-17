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
    repositories::free_inventory_list_s3::{FreeListNamespace, S3FreeInventoryLister},
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
    // DROP-07b: the listing is bounded and keeps an opaque namespace-bound
    // continuation token. No LIST result authorizes a DELETE.
    let second_key = "private/free-chapters/70001/70002.md";
    client
        .put_object()
        .bucket(BUCKET)
        .key(second_key)
        .body(ByteStream::from(b"other".to_vec()))
        .send()
        .await
        .unwrap();
    let lister = S3FreeInventoryLister::new(client.clone());
    let first_page = lister
        .list_page(BUCKET, FreeListNamespace::Single, None, 1)
        .await
        .unwrap();
    assert_eq!(first_page.listed.len(), 1);
    assert_eq!(first_page.listed[0].object_key, KEY);
    assert_eq!(first_page.listed[0].size_bytes, actual.len() as u64);
    assert!(first_page.listed[0].modified_at_unix.is_some());
    let next_token = first_page.next_token.unwrap();
    let second_page = lister
        .list_page(BUCKET, FreeListNamespace::Single, Some(&next_token), 1)
        .await
        .unwrap();
    assert_eq!(second_page.listed.len(), 1);
    assert_eq!(second_page.listed[0].object_key, second_key);
    assert_eq!(second_page.next_token, None);

    const PACK_KEY: &str = "private/free-packs/ci/drop07-list-immutable.zst";
    const MANIFEST_KEY: &str = "private/free-packs/ci/drop07-list-immutable.manifest.json";
    for name in [PACK_KEY, MANIFEST_KEY] {
        client
            .put_object()
            .bucket(BUCKET)
            .key(name)
            .body(ByteStream::from(b"immutable".to_vec()))
            .send()
            .await
            .unwrap();
    }
    let packed = lister
        .list_page(BUCKET, FreeListNamespace::Pack, None, 8)
        .await
        .unwrap();
    assert_eq!(packed.listed.len(), 2);
    assert!(packed
        .listed
        .iter()
        .any(|entry| entry.object_key == PACK_KEY));
    assert!(packed
        .listed
        .iter()
        .any(|entry| entry.object_key == MANIFEST_KEY));
    assert!(packed.next_token.is_none());
    assert_eq!(
        lister
            .list_page(BUCKET, FreeListNamespace::Pack, None, 129)
            .await,
        Err(StorageError::Invalid)
    );

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
                "private/free-chapters/70001/70003.md",
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
