//! S3-04/05: isolated provider evidence using loopback MinIO.
//! Not a paid reader, entitlement grant, production credential or public route.
#![cfg(feature = "s3")]

use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    Client,
};
use std::{process::Command, time::Duration};
use tor_contracts::paid_delivery::{
    ChapterVersionRef, EncryptedObjectKind, ObjectId, ObjectRef, PositiveId,
};
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
        .args([
            "--silent",
            "--output",
            "/dev/null",
            "--write-out",
            "%{http_code}",
            url,
        ])
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
        endpoint.starts_with("http://127.0.0.1:") || endpoint.starts_with("http://localhost:"),
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

    let public_url = format!("{endpoint}/{BUCKET}/private/chapter-versions/{ID}.enc");
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
    assert_eq!(
        store.get_verified(&forged).await,
        Err(StorageError::Corrupt)
    );

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
    assert_eq!(
        store.get_verified(&record).await,
        Err(StorageError::Corrupt)
    );
}

#[test]
fn private_bucket_cannot_be_selected_from_client_controlled_paths() {
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            "never-used",
            "never-used",
            None,
            None,
            "unit",
        ))
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

#[tokio::test]
#[ignore = "requires disposable loopback MinIO only"]
async fn g_09_reconciliation_2501_private_list_pages() {
    use std::collections::HashSet;
    use tokio::task::JoinSet;
    use tor_storage::services::reconciliation::{PrivateInventoryLister, PrivateScope};

    let endpoint = std::env::var("TOR_TEST_S3_ENDPOINT").expect("loopback MinIO URL");
    assert!(
        endpoint.starts_with("http://127.0.0.1:") || endpoint.starts_with("http://localhost:"),
        "never use a production S3 endpoint"
    );
    let access = std::env::var("TOR_TEST_S3_ACCESS_KEY").unwrap();
    let secret = std::env::var("TOR_TEST_S3_SECRET_KEY").unwrap();
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            access,
            secret,
            None,
            None,
            "loopback-test",
        ))
        .endpoint_url(&endpoint)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    let client = Client::from_conf(config);
    let bucket = "tor-private-inventory-ci";
    client
        .create_bucket()
        .bucket(bucket)
        .send()
        .await
        .expect("create isolated private bucket");
    let store = S3PrivateStore::new(client.clone(), bucket.to_owned()).unwrap();

    // Actual provider pagination beyond two S3 1000-key pages, not mocked.
    let mut tasks = JoinSet::new();
    for number in 1..=2501_u32 {
        let worker = client.clone();
        tasks.spawn(async move {
            let key =
                format!("private/chapter-versions/{number:08x}-dcb1-4c1a-a000-{number:012x}.enc");
            worker
                .put_object()
                .bucket("tor-private-inventory-ci")
                .key(key)
                .body(ByteStream::from_static(b"inventory-test-only"))
                .send()
                .await
                .expect("disposable test object");
        });
        if tasks.len() >= 32 {
            tasks.join_next().await.unwrap().unwrap();
        }
    }
    while let Some(result) = tasks.join_next().await {
        result.unwrap();
    }

    let mut continuation: Option<String> = None;
    let mut pages = Vec::new();
    let mut keys = HashSet::new();
    loop {
        let page = store
            .list_private_page(PrivateScope::Chapter, continuation.as_deref())
            .await
            .unwrap();
        pages.push(page.objects.len());
        for object in page.objects {
            assert!(object.key.starts_with(PrivateScope::Chapter.prefix()));
            assert!(keys.insert(object.key), "duplicate LIST key across pages");
        }
        if let Some(next) = page.continuation {
            assert_ne!(continuation.as_deref(), Some(next.as_str()));
            continuation = Some(next);
        } else {
            break;
        }
        assert!(pages.len() <= 3, "LIST cursor must advance");
    }
    assert_eq!(pages, vec![1000, 1000, 501]);
    assert_eq!(keys.len(), 2501);
}

#[tokio::test]
#[ignore = "requires disposable loopback MinIO only"]
async fn g_09_reverse_s3_head_distinguishes_missing_mismatch_and_outage() {
    use tor_storage::services::reconciliation::{PrivateRegisteredHead, RegisteredHeadResult};

    let endpoint = std::env::var("TOR_TEST_S3_ENDPOINT").expect("MinIO URL");
    assert!(
        endpoint.starts_with("http://127.0.0.1:") || endpoint.starts_with("http://localhost:"),
        "never test against a production S3 endpoint"
    );
    let access = std::env::var("TOR_TEST_S3_ACCESS_KEY").unwrap();
    let secret = std::env::var("TOR_TEST_S3_SECRET_KEY").unwrap();
    let configure = |key: String, password: String| {
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new(key, password, None, None, "loopback-test"))
            .endpoint_url(&endpoint)
            .force_path_style(true)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .build()
    };
    let client = Client::from_conf(configure(access, secret));
    let bucket = "tor-private-reverse-head-ci";
    client.create_bucket().bucket(bucket).send().await.unwrap();
    let store = S3PrivateStore::new(client.clone(), bucket.into()).unwrap();
    let id = ObjectId::parse("9b000000-0000-4000-8000-000000000001").unwrap();
    let body = b"immutable ciphertext metadata test bytes";
    let record = object(body, id);
    client
        .put_object()
        .bucket(bucket)
        .key(format!(
            "private/chapter-versions/{}.enc",
            record.object_id.as_str()
        ))
        .content_type("application/octet-stream")
        .metadata("sha256", hex::encode(record.sha256_ciphertext))
        .body(ByteStream::from(body.to_vec()))
        .send()
        .await
        .unwrap();
    assert_eq!(
        store.head_registered(&record).await,
        Ok(RegisteredHeadResult::Present)
    );
    let forged = object(
        b"forged ciphertext metadata exactly here",
        record.object_id.clone(),
    );
    assert_eq!(
        store.head_registered(&forged).await,
        Ok(RegisteredHeadResult::MetadataMismatch)
    );
    let missing = object(
        body,
        ObjectId::parse("9b000000-0000-4000-8000-000000000002").unwrap(),
    );
    assert_eq!(
        store.head_registered(&missing).await,
        Ok(RegisteredHeadResult::Missing)
    );

    // Authentication failure must NOT be classified as a missing object.
    let wrong = Client::from_conf(configure(
        "invalid-ci-key".into(),
        "invalid-ci-secret".into(),
    ));
    let blocked = S3PrivateStore::new(wrong, bucket.into()).unwrap();
    assert_eq!(
        blocked.head_registered(&record).await,
        Err(StorageError::Unavailable)
    );
}

#[tokio::test]
#[ignore = "requires disposable loopback MinIO only"]
async fn g09_bounded_body_get_hash_proves_actual_ciphertext_without_leaking_bytes() {
    use tor_storage::services::reconciliation::{
        PrivateRegisteredBodyProbe, RegisteredBodyResult, G09_BODY_GET_MAX_BYTES,
    };

    let endpoint = std::env::var("TOR_TEST_S3_ENDPOINT").expect("loopback MinIO URL");
    assert!(
        endpoint.starts_with("http://127.0.0.1:") || endpoint.starts_with("http://localhost:"),
        "never issue test GET requests to production"
    );
    let access = std::env::var("TOR_TEST_S3_ACCESS_KEY").unwrap();
    let secret = std::env::var("TOR_TEST_S3_SECRET_KEY").unwrap();
    let configure = |user: String, password: String| {
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new(
                user,
                password,
                None,
                None,
                "loopback-test",
            ))
            .endpoint_url(&endpoint)
            .force_path_style(true)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .build()
    };
    let client = Client::from_conf(configure(access, secret));
    let bucket = "tor-private-g09-body-ci";
    client.create_bucket().bucket(bucket).send().await.unwrap();
    let store = S3PrivateStore::new(client.clone(), bucket.to_owned()).unwrap();

    let id = ObjectId::parse("a23052ff-725e-4c15-b846-52da55aa1234").unwrap();
    let ciphertext = b"minio-isolated-ciphertext-body-sample";
    let valid = object(ciphertext, id.clone());
    assert_eq!(store.put_immutable(&valid, ciphertext).await, Ok(()));
    assert_eq!(
        store.probe_registered_body(&valid).await,
        Ok(RegisteredBodyResult::Verified)
    );
    // Identical, trusted body but wrong recorded digest: cannot be healthy.
    let wrong = ObjectRef::new(
        id,
        EncryptedObjectKind::Chapter,
        sha256_bytes(b"wrong-expected-ciphertext"),
        ciphertext.len() as u64,
    )
    .unwrap();
    assert_eq!(
        store.probe_registered_body(&wrong).await,
        Ok(RegisteredBodyResult::BodyMismatch)
    );

    // Actual bytes match but GET metadata is tampered with.
    let metadata_id = ObjectId::parse("a23052ff-725e-4c15-b846-52da55aa5678").unwrap();
    let metadata_ref = object(ciphertext, metadata_id.clone());
    client
        .put_object()
        .bucket(bucket)
        .key(format!(
            "private/chapter-versions/{}.enc",
            metadata_id.as_str()
        ))
        .content_type("application/octet-stream")
        .metadata("sha256", "incorrect-metadata")
        .body(ByteStream::from_static(ciphertext))
        .send()
        .await
        .unwrap();
    assert_eq!(
        store.probe_registered_body(&metadata_ref).await,
        Ok(RegisteredBodyResult::MetadataMismatch)
    );

    // 404 is evidence of missing, but bad credentials are not.
    let missing_id = ObjectId::parse("a23052ff-725e-4c15-b846-52da55aa9999").unwrap();
    let missing = object(ciphertext, missing_id);
    assert_eq!(
        store.probe_registered_body(&missing).await,
        Ok(RegisteredBodyResult::Missing)
    );
    let invalid_client = Client::from_conf(configure(
        "incorrect-username".to_owned(),
        "incorrect-password".to_owned(),
    ));
    let unauthorized = S3PrivateStore::new(invalid_client, bucket.to_owned()).unwrap();
    assert_eq!(
        unauthorized.probe_registered_body(&valid).await,
        Err(StorageError::Unavailable)
    );

    // Oversize must be refused before any GET; 16 MiB publication max remains unchanged.
    let large = ObjectRef::new(
        ObjectId::parse("a23052ff-725e-4c15-b846-52da55aa3210").unwrap(),
        EncryptedObjectKind::Chapter,
        valid.sha256_ciphertext,
        G09_BODY_GET_MAX_BYTES + 1,
    )
    .unwrap();
    assert_eq!(
        store.probe_registered_body(&large).await,
        Err(StorageError::Invalid)
    );
}
