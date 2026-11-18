//! DROP-07a: actual private MinIO GET/SHA evidence, never remote provider.
#![cfg(feature = "s3")]

#[path = "fixtures/drop09_version_read.rs"]
mod version_read;

use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    types::{BucketVersioningStatus, VersioningConfiguration},
    Client,
};
use sha2::{Digest, Sha256};
use tor_storage::{
    domain::provider::StorageError,
    repositories::free_expected_inventory_s3::{FreeObjectEvidence, S3FreeExpectedObjectVerifier},
    repositories::free_inventory_list_s3::{FreeListNamespace, S3FreeInventoryLister},
    repositories::free_pack_write_s3::S3FreePackWriter,
    services::{
        free_chapter::FreeChapterObject, free_pack_read::decode_exact_frame,
        free_pack_write::prepare_free_pack,
    },
};

fn loopback_client() -> Client {
    let endpoint = std::env::var("TOR_TEST_S3_ENDPOINT").expect("local MinIO");
    assert!(
        endpoint
            .strip_prefix("http://127.0.0.1:")
            .or_else(|| endpoint.strip_prefix("http://localhost:"))
            .is_some_and(|port| port.parse::<u16>().is_ok()),
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
    Client::from_conf(config)
}

#[tokio::test]
#[ignore = "requires disposable loopback MinIO, read-only verifier"]
async fn drop_07a_actual_bytes_are_verified_without_missing_or_gc_claims() {
    let client = loopback_client();
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

#[tokio::test]
#[ignore = "requires disposable versioned loopback MinIO"]
async fn drop_09_version_specific_head_get_and_range_survive_a_new_current_object() {
    let client = loopback_client();
    const BUCKET: &str = "tor-private-drop09-version-ci";
    const PACK: &str = "private/free-packs/ci/drop09-versioned-read.zst";
    client.create_bucket().bucket(BUCKET).send().await.unwrap();
    client
        .put_bucket_versioning()
        .bucket(BUCKET)
        .versioning_configuration(
            VersioningConfiguration::builder()
                .status(BucketVersioningStatus::Enabled)
                .build(),
        )
        .send()
        .await
        .unwrap();
    let text = "saved FREE V1 in a previous S3 object version";
    let sources = [(
        FreeChapterObject::for_markdown(410, 411, text).unwrap(),
        text.as_bytes().to_vec(),
    )];
    let pack = prepare_free_pack(BUCKET, PACK, &sources).unwrap();
    S3FreePackWriter::new(client.clone())
        .upload_and_verify(&pack)
        .await
        .unwrap();
    let versions = version_read::current_version(&client, BUCKET, PACK).await;
    let manifest = &pack.target().manifest_key;
    let manifest_version = version_read::current_version(&client, BUCKET, manifest).await;

    // Force a different current version on this disposable key. A backend
    // that ignored versionId could no longer return the expected frame.
    let mut changed = pack.pack_bytes().to_vec();
    changed[0] ^= 1;
    client
        .put_object()
        .bucket(BUCKET)
        .key(PACK)
        .body(ByteStream::from(changed))
        .send()
        .await
        .unwrap();
    assert!(version_read::current_version(&client, BUCKET, PACK).await != versions);
    version_read::assert_exact_object(&client, BUCKET, PACK, &versions, pack.pack_bytes()).await;
    version_read::assert_exact_object(
        &client,
        BUCKET,
        manifest,
        &manifest_version,
        pack.manifest_bytes(),
    )
    .await;
    let reference = version_read::pack_frame(&pack, 0);
    let range = version_read::exact_version_range(&client, &reference, &versions).await;
    assert!(decode_exact_frame(&reference, &range).unwrap() == text.as_bytes());
    println!("DROP09_MINIO_VERSION_READ_OK");
}
