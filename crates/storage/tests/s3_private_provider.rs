//! S3-04/05: isolated provider evidence using loopback MinIO.
//! Not a paid reader, entitlement grant, production credential or public route.
#![cfg(feature = "s3")]

use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    Client,
};
use std::{process::Command, time::Duration};
use tokio::io::AsyncReadExt;
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
    let after_rejected_overwrite = store.get_verified(&record).await;
    if let Err(error) = &after_rejected_overwrite {
        // Only on failure: do not insert extra reads between the conflicting
        // conditional PUT and the original asserted GET (which could hide a race).
        // Never log the S3 object key, signed URLs, credentials or ciphertext.
        use aws_sdk_s3::error::ProvideErrorMetadata;
        eprintln!("G09_DIAGNOSTIC verified_get={error:?}");
        let key = format!("private/chapter-versions/{ID}.enc");
        match client.head_object().bucket(BUCKET).key(&key).send().await {
            Ok(head) => eprintln!(
                "G09_DIAGNOSTIC raw_head=ok length_matches={} digest_metadata_matches={}",
                head.content_length() == Some(record.size_bytes as i64),
                head.metadata()
                    .and_then(|fields| fields.get("sha256"))
                    .is_some_and(|digest| digest == &hex::encode(record.sha256_ciphertext))
            ),
            Err(fault) => eprintln!(
                "G09_DIAGNOSTIC raw_head=error category={} service_code={}",
                if fault.as_service_error().is_some() {
                    "s3-service"
                } else {
                    "sdk-transport-or-response"
                },
                fault
                    .as_service_error()
                    .and_then(|value| value.code())
                    .unwrap_or("none")
            ),
        }
        match client.get_object().bucket(BUCKET).key(&key).send().await {
            Ok(get) => {
                let content_length_matches = get.content_length() == Some(record.size_bytes as i64);
                let metadata_matches = get
                    .metadata()
                    .and_then(|fields| fields.get("sha256"))
                    .is_some_and(|digest| digest == &hex::encode(record.sha256_ciphertext));
                let mut bytes = Vec::new();
                let mut reader = get.body.into_async_read().take(record.size_bytes + 1);
                match reader.read_to_end(&mut bytes).await {
                    Ok(_) => eprintln!(
                        "G09_DIAGNOSTIC raw_get=ok length_matches={} digest_metadata_matches={} body_length_matches={} body_sha_matches={}",
                        content_length_matches,
                        metadata_matches,
                        bytes.len() as u64 == record.size_bytes,
                        sha256_bytes(&bytes) == record.sha256_ciphertext
                    ),
                    Err(_) => eprintln!(
                        "G09_DIAGNOSTIC raw_get=body_stream_error length_matches={} digest_metadata_matches={}",
                        content_length_matches,
                        metadata_matches
                    ),
                }
            }
            Err(fault) => eprintln!(
                "G09_DIAGNOSTIC raw_get=error category={} service_code={}",
                if fault.as_service_error().is_some() {
                    "s3-service"
                } else {
                    "sdk-transport-or-response"
                },
                fault
                    .as_service_error()
                    .and_then(|value| value.code())
                    .unwrap_or("none")
            ),
        }
    }
    assert_eq!(
        after_rejected_overwrite.unwrap(),
        encrypted.ciphertext,
        "rejected immutable overwrite must not destroy the original paid edition"
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

#[tokio::test]
#[ignore = "requires disposable loopback MinIO only"]
async fn s005_real_s3_head_transport_failure_never_returns_presigned_grant() {
    let endpoint = std::env::var("TOR_TEST_S3_ENDPOINT").expect("isolated MinIO URL");
    assert!(
        endpoint.starts_with("http://127.0.0.1:") || endpoint.starts_with("http://localhost:"),
        "only loopback S3 may be used for fault injection"
    );
    let access = std::env::var("TOR_TEST_S3_ACCESS_KEY").expect("test access");
    let secret = std::env::var("TOR_TEST_S3_SECRET_KEY").expect("test secret");
    let configure = |endpoint: &str| {
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("us-east-1"))
            .credentials_provider(Credentials::new(
                access.clone(),
                secret.clone(),
                None,
                None,
                "loopback-test",
            ))
            .endpoint_url(endpoint)
            .force_path_style(true)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .build()
    };
    let bucket = "tor-private-s005-outage-ci";
    let live = Client::from_conf(configure(&endpoint));
    live.create_bucket().bucket(bucket).send().await.unwrap();
    let working = S3PrivateStore::new(live, bucket.to_owned()).unwrap();
    let record = object(
        b"S005 isolated S3 transport failure, not a paid chapter",
        ObjectId::parse("f0050000-0000-4000-8000-000000000001").unwrap(),
    );
    working
        .put_immutable(
            &record,
            b"S005 isolated S3 transport failure, not a paid chapter",
        )
        .await
        .unwrap();
    assert_eq!(working.head_verified(&record).await, Ok(true));

    // Use a separate local TCP endpoint which accepts each connection but
    // closes before an S3 HEAD response. Real MinIO stays healthy for all
    // neighbouring acceptance tests; this is a real transport disconnect
    // through the AWS SDK, not a mocked StorageError.
    let breaker = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let unavailable_endpoint = format!("http://{}", breaker.local_addr().unwrap());
    let disconnect = tokio::spawn(async move {
        while let Ok((socket, _)) = breaker.accept().await {
            drop(socket);
        }
    });
    let offline = S3PrivateStore::new(
        Client::from_conf(configure(&unavailable_endpoint)),
        bucket.to_owned(),
    )
    .unwrap();
    let failed = tokio::time::timeout(
        Duration::from_secs(8),
        offline.presign_verified(&record, Duration::from_secs(90)),
    )
    .await
    .expect("S3 transport outage must be bounded");
    assert_eq!(
        failed,
        Err(StorageError::Unavailable),
        "connection reset is not a missing object or a signed success"
    );
    disconnect.abort();

    // After the isolated transport failure, the genuine private object and
    // its 90-second grant remain available without any repair to the record.
    assert_eq!(working.head_verified(&record).await, Ok(true));
    let live_url = working
        .presign_verified(&record, Duration::from_secs(90))
        .await
        .unwrap();
    let signed = serde_json::to_value(live_url).unwrap();
    assert_eq!(
        curl_status(signed.as_str().unwrap()),
        "200",
        "a recovered private S3 signer still grants the exact object"
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

    // A corrupted S3 writer can forge *correct* SHA metadata while storing
    // different bytes. HEAD would pass, but a complete bounded GET must fail.
    let forged_id = ObjectId::parse("a23052ff-725e-4c15-b846-52da55aa9876").unwrap();
    let expected_body = b"correct-but-never-uploaded-ciphertext";
    let stored_body = b"damaged-but-same-length-ciphertext-xx";
    assert_eq!(expected_body.len(), stored_body.len());
    let forged_ref = object(expected_body, forged_id.clone());
    client
        .put_object()
        .bucket(bucket)
        .key(format!(
            "private/chapter-versions/{}.enc",
            forged_id.as_str()
        ))
        .content_type("application/octet-stream")
        .metadata("sha256", hex::encode(forged_ref.sha256_ciphertext))
        .body(ByteStream::from_static(stored_body))
        .send()
        .await
        .unwrap();
    assert_eq!(
        store.probe_registered_body(&forged_ref).await,
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

/// #533: failure-only probes run after the *first* strict GET assertion has
/// already failed. They must never be used as a retry before that assertion.
async fn g09_conflict_repro_failure_evidence(client: &Client, bucket: &str, record: &ObjectRef) {
    use aws_sdk_s3::error::ProvideErrorMetadata;

    let key = format!(
        "private/chapter-versions/{}.enc",
        record.object_id.as_str()
    );
    match client.head_object().bucket(bucket).key(&key).send().await {
        Ok(head) => eprintln!(
            "G09_REPRO raw_head=ok size_matches={} sha_metadata_matches={}",
            head.content_length() == Some(record.size_bytes as i64),
            head.metadata()
                .and_then(|m| m.get("sha256"))
                .is_some_and(|sha| sha == &hex::encode(record.sha256_ciphertext))
        ),
        Err(error) => eprintln!(
            "G09_REPRO raw_head=error category={} service_code={}",
            if error.as_service_error().is_some() {
                "s3-service"
            } else {
                "sdk-transport-or-response"
            },
            error
                .as_service_error()
                .and_then(|value| value.code())
                .unwrap_or("none")
        ),
    }
    match client.get_object().bucket(bucket).key(&key).send().await {
        Ok(get) => {
            let metadata_matches = get
                .metadata()
                .and_then(|m| m.get("sha256"))
                .is_some_and(|sha| sha == &hex::encode(record.sha256_ciphertext));
            match get.body.collect().await {
                Ok(body) => {
                    let bytes = body.into_bytes();
                    eprintln!(
                        "G09_REPRO raw_get=ok length_matches={} metadata_matches={} sha_matches={}",
                        bytes.len() as u64 == record.size_bytes,
                        metadata_matches,
                        sha256_bytes(&bytes) == record.sha256_ciphertext
                    );
                }
                Err(_) => eprintln!(
                    "G09_REPRO raw_get=stream_error metadata_matches={metadata_matches}"
                ),
            }
        }
        Err(error) => eprintln!(
            "G09_REPRO raw_get=error category={} service_code={}",
            if error.as_service_error().is_some() {
                "s3-service"
            } else {
                "sdk-transport-or-response"
            },
            error
                .as_service_error()
                .and_then(|value| value.code())
                .unwrap_or("none")
        ),
    }
}

async fn g09_conflict_first_get_round(
    client: &Client,
    store: &S3PrivateStore,
    bucket: &str,
    round: u16,
    mode: &str,
) {
    // Every round uses a distinct canonical immutable key; background
    // traffic never writes this key or shares its ObjectId.
    let id = ObjectId::parse(&format!("c0f10000-0000-4000-8000-{round:012x}")).unwrap();
    let original = seal_chapter(
        version(),
        id.clone(),
        &format!("original private synthetic fixture round {round}"),
        &ContentKek::from_bytes([53; 32]),
    )
    .unwrap();
    let expected = object(&original.ciphertext, id.clone());
    assert_eq!(
        store.put_immutable(&expected, &original.ciphertext).await,
        Ok(()),
        "first immutable PUT must succeed in {mode} round {round}"
    );
    let competing = seal_chapter(
        version(),
        id.clone(),
        &format!("different immutable writer round {round}"),
        &ContentKek::from_bytes([53; 32]),
    )
    .unwrap();
    let forged = object(&competing.ciphertext, id);
    assert_ne!(forged.sha256_ciphertext, expected.sha256_ciphertext);
    assert_eq!(
        store.put_immutable(&forged, &competing.ciphertext).await,
        Err(StorageError::Conflict),
        "conditional competing PUT must fail in {mode} round {round}"
    );

    // CRITICAL: the very next request touching this target is the first
    // exact GET. No HEAD, wait or retry between the 412 and this GET.
    let first_get = store.get_verified(&expected).await;
    if let Err(error) = &first_get {
        eprintln!("G09_REPRO mode={mode} round={round} first_get={error:?}");
        g09_conflict_repro_failure_evidence(client, bucket, &expected).await;
    }
    assert!(
        first_get
            .as_ref()
            .is_ok_and(|bytes| bytes == &original.ciphertext),
        "first GET after rejected overwrite failed integrity in {mode} round {round}"
    );
}

/// #533: bounded one-key PUT -> rejected conflicting PUT -> first exact GET.
/// Both the quiet baseline and parallel traffic use disposable loopback MinIO.
/// This diagnostic does not alter the storage adapter or retry a failed GET.
#[tokio::test]
#[ignore = "only against disposable loopback MinIO from s3-provider"]
async fn g09_first_get_after_immutable_conflict_isolated_and_under_load() {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    let endpoint = std::env::var("TOR_TEST_S3_ENDPOINT").expect("loopback MinIO required");
    assert!(
        endpoint.starts_with("http://127.0.0.1:")
            || endpoint.starts_with("http://localhost:"),
        "never target external S3"
    );
    let access = std::env::var("TOR_TEST_S3_ACCESS_KEY").expect("test-only access key");
    let secret = std::env::var("TOR_TEST_S3_SECRET_KEY").expect("test-only secret");
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            access,
            secret,
            None,
            None,
            "g09-loopback-only",
        ))
        .endpoint_url(&endpoint)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    let client = Client::from_conf(config);
    let bucket = "tor-g09-conflict-repro-ci";
    client
        .create_bucket()
        .bucket(bucket)
        .send()
        .await
        .expect("new disposable G09 bucket");
    let store = S3PrivateStore::new(client.clone(), bucket.to_owned()).unwrap();

    for round in 1..=8 {
        g09_conflict_first_get_round(&client, &store, bucket, round, "isolated").await;
    }

    // Independent, bounded GET traffic on four other keys. No background
    // request can touch a target key and all operations must complete.
    let counter = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(tokio::sync::Barrier::new(5));
    let mut workers = Vec::new();
    for worker in 0..4 {
        let load_key = format!("g09-load-fixture-{worker}");
        client
            .put_object()
            .bucket(bucket)
            .key(&load_key)
            .body(ByteStream::from_static(b"bounded-g09-load-fixture"))
            .send()
            .await
            .expect("create isolated background key");
        let background = client.clone();
        let bg_bucket = bucket.to_owned();
        let bg_counter = Arc::clone(&counter);
        let bg_barrier = Arc::clone(&barrier);
        workers.push(tokio::spawn(async move {
            bg_barrier.wait().await;
            for _ in 0..96 {
                let response = background
                    .get_object()
                    .bucket(&bg_bucket)
                    .key(&load_key)
                    .send()
                    .await
                    .expect("background GET must succeed");
                let body = response.body.collect().await.expect("background body");
                assert!(
                    &body.into_bytes()[..] == b"bounded-g09-load-fixture",
                    "background fixture integrity mismatch"
                );
                bg_counter.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }
    barrier.wait().await;
    for round in 9..=24 {
        g09_conflict_first_get_round(&client, &store, bucket, round, "parallel").await;
    }
    for worker in workers {
        worker.await.expect("bounded load task");
    }
    assert_eq!(counter.load(Ordering::Relaxed), 4 * 96);
    eprintln!("G09_REPRO baseline=8 parallel=16 load_gets=384 first_get_failures=0");
}
