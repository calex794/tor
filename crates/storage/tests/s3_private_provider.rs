//! S3-04/05: isolated provider evidence using loopback MinIO.
//! Not a paid reader, entitlement grant, production credential or public route.
#![cfg(feature = "s3")]

use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    Client,
};
use std::{process::Command, time::Duration};
use tor_contracts::paid_delivery::{ChapterVersionRef, EncryptedObjectKind, ObjectId, ObjectRef, PositiveId};
use tor_storage::{
    domain::provider::StorageError,
    repositories::s3::S3PrivateStore,
    services::{
        crypto::{seal_chapter, sha256_bytes, ContentKek},
        provider::PrivateObjectStore,
    },
};

const BUCKET: &str = "tor-private-chapter-ci";
const ID: &str = "2ad66e0b-bc2f-4cf3-b857-97e201f40d5a";

fn object(bytes: &[u8], id: ObjectId) -> ObjectRef {
    ObjectRef::new(
        id,
        EncryptedObjectKind::Chapter,
        sha256_bytes(bytes),
        bytes.len() as u64,
    )
    .unwrap()
}

fn chapter_id() -> ObjectId {
    ObjectId::parse(ID).unwrap()
}

fn version() -> ChapterVersionRef {
    ChapterVersionRef {
        chapter_id: PositiveId::new(7781).unwrap(),
        version_id: PositiveId::new(7791).unwrap(),
    }
}

fn curl_status(url: &str) -> String {
    let result = Command::new("curl")
        .args(["--silent", "--output", "/dev/null", "--write-out", "%{http_code}", url])
        .output()
        .expect("curl installed in CI");
    assert!(result.status.success(), "HTTP probe failed");
    String::from_utf8(result.stdout).unwrap()
}

#[tokio::test]
#[ignore = "only on ephemeral loopback MinIO from storage-s3.yml"]
async fn g_09_private_s3_immutable_signed_get_and_integrity() {
    let endpoint = std::env::var("TOR_TEST_S3_ENDPOINT").expect("MinIO URL");
    assert!(
        endpoint.starts_with("http://127.0.0.1:")
            || endpoint.starts_with("http://localhost:"),
        "this test cannot run against production S3"
    );
    let key = std::env::var("TOR_TEST_S3_ACCESS_KEY").expect("test-only access");
    let secret = std::env::var("TOR_TEST_S3_SECRET_KEY").expect("test-only secret");
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(key, secret, None, None, "loopback-test"))
        .endpoint_url(&endpoint)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    let client = Client::from_conf(config);
    client
        .create_bucket()
        .bucket(BUCKET)
        .send()
        .await
        .expect("create private, non-public bucket");
    let store = S3PrivateStore::new(client.clone(), BUCKET.to_owned()).unwrap();

    let encrypted = seal_chapter(
        version(),
        chapter_id(),
        "Глава только для владельца: А😀Б\n\n第二章",
        &ContentKek::from_bytes([53; 32]),
    )
    .unwrap();
    let record = object(&encrypted.ciphertext, chapter_id());
    assert_eq!(
        store.put_immutable(&record, &encrypted.ciphertext).await,
        Ok(())
    );
    assert_eq!(store.head_verified(&record).await, Ok(true));
    assert_eq!(
        store.get_verified(&record).await.unwrap(),
        encrypted.ciphertext
    );

    let public_url = format!(
        "{endpoint}/{BUCKET}/private/chapter-versions/{ID}.enc"
    );
    assert_eq!(
        curl_status(&public_url),
        "403",
        "anonymous URL must never access private ciphertext"
    );

    // S3 must refuse an overwrite even when the second writer claims a
    // valid but *different* digest for the same immutable object ID.
    let other = seal_chapter(
        version(),
        chapter_id(),
        "Overwriting this version would revoke a bought V1",
        &ContentKek::from_bytes([53; 32]),
    )
    .unwrap();
    let forged = object(&other.ciphertext, chapter_id());
    assert_eq!(
        store.put_immutable(&forged, &other.ciphertext).await,
        Err(StorageError::Conflict)
    );
    assert_eq!(
        store.get_verified(&record).await.unwrap(),
        encrypted.ciphertext
    );
    assert_eq!(store.head_verified(&forged).await, Ok(false));
    assert_eq!(store.get_verified(&forged).await, Err(StorageError::Corrupt));

    for ttl in [
        Duration::ZERO,
        Duration::from_secs(29),
        Duration::from_secs(121),
    ] {
        assert!(matches!(
            store.presign_verified(&record, ttl).await,
            Err(StorageError::Invalid)
        ));
    }
    let signed = store
        .presign_verified(&record, Duration::from_secs(90))
        .await
        .unwrap();
    assert!(!format!("{signed:?}").contains("X-Amz-"));
    let token = serde_json::to_value(signed).unwrap();
    let url = token.as_str().expect("signed URL string");
    assert!(url.contains("X-Amz-Signature="));
    assert_eq!(curl_status(url), "200", "temporary signed GET works");
    let fetched = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", url])
        .output()
        .expect("curl installed");
    assert!(fetched.status.success(), "signed GET must succeed");
    assert_eq!(sha256_bytes(&fetched.stdout), record.sha256_ciphertext);
    assert_eq!(fetched.stdout, encrypted.ciphertext);

    let stale = ObjectRef::new(
        ObjectId::parse("77777777-7777-4777-8777-777777777777").unwrap(),
        EncryptedObjectKind::Chapter,
        record.sha256_ciphertext,
        record.size_bytes,
    )
    .unwrap();
    assert_eq!(
        store.get_verified(&stale).await,
        Err(StorageError::Unavailable)
    );

    // A compromised storage writer can forge *HEAD* metadata. The G reader
    // must verify GET bytes before PG attach, and browsers verify SHA again.
    let mut damaged = encrypted.ciphertext;
    damaged[0] ^= 1;
    client
        .put_object()
        .bucket(BUCKET)
        .key(format!("private/chapter-versions/{ID}.enc"))
        .content_type("application/octet-stream")
        .metadata("sha256", hex::encode(record.sha256_ciphertext))
        .body(ByteStream::from(damaged))
        .send()
        .await
        .expect("simulate corrupted private object with test-only owner");
    assert_eq!(store.head_verified(&record).await, Ok(true));
    assert_eq!(store.get_verified(&record).await, Err(StorageError::Corrupt));
}

#[test]
fn private_bucket_cannot_be_selected_from_client_controlled_paths() {
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new("never-used", "never-used", None, None, "unit"))
        .build();
    for bucket in [
        "",
        "public",
        "public-books",
        "../private",
        "tor_private",
        "tor.private",
        "tor-private/",
        "TEST",
        "with space",
    ] {
        assert!(matches!(
            S3PrivateStore::new(Client::from_conf(config.clone()), bucket.to_owned()),
            Err(StorageError::Invalid)
        ));
    }
}
