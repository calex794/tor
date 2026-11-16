//! DROP-R3a: real private MinIO Range GET only (no HTTP gateway or users).
//! Kept isolated from production and used by the dedicated S3-provider lane.
#![cfg(feature = "s3")]

use aws_sdk_s3::{
    config::{retry::RetryConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    Client,
};
use std::{future::ready, process::Command};
use tor_storage::{
    domain::provider::StorageError,
    repositories::{
        free_pack_proof_s3::S3PackProofStore,
        free_pack_range_s3::{response_metadata_matches, S3PackFrameStore},
    },
    services::{
        crypto::sha256_bytes,
        free_chapter::FreeChapterObject,
        free_pack_proof::PackProofTarget,
        free_pack_read::read_reauthorized_pack,
        free_placement::{plan_after_acl, FreePhysicalPlacement},
    },
};

fn test_client() -> (Client, String) {
    let endpoint = std::env::var("TOR_TEST_S3_ENDPOINT").expect("local MinIO");
    assert!(
        endpoint.starts_with("http://127.0.0.1:") || endpoint.starts_with("http://localhost:"),
        "never use a real S3 provider in this test"
    );
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .credentials_provider(Credentials::new(
            std::env::var("TOR_TEST_S3_ACCESS_KEY").unwrap(),
            std::env::var("TOR_TEST_S3_SECRET_KEY").unwrap(),
            None,
            None,
            "drop-r3a-loopback",
        ))
        .endpoint_url(endpoint.clone())
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .build();
    (Client::from_conf(config), endpoint)
}

#[tokio::test]
#[ignore = "requires disposable 127.0.0.1 MinIO, no remote provider"]
async fn drop_r3a_real_private_minio_range_reads_only_selected_zstd_frame() {
    let (client, endpoint) = test_client();
    const BUCKET: &str = "tor-private-drop-r3a-ci";
    let object_key = "private/free-packs/ci/verified-r3a-pair.zst";
    client.create_bucket().bucket(BUCKET).send().await.unwrap();

    let earlier = "Чужая соседняя глава: нельзя выдавать";
    let entitled = "Разрешённая FREE глава, Unicode 中文😀";
    let first = zstd::bulk::compress(earlier.as_bytes(), 3).unwrap();
    let second = zstd::bulk::compress(entitled.as_bytes(), 3).unwrap();
    let seek_start = first.len() + second.len();
    let mut pack = first.clone();
    pack.extend_from_slice(&second);

    // Standards-compatible seekable table: magic, 2 frame sizes, footer.
    let mut table = Vec::new();
    table.extend_from_slice(&0x184D2A5E_u32.to_le_bytes());
    table.extend_from_slice(&(25_u32).to_le_bytes());
    for (compressed, raw) in [(&first, earlier), (&second, entitled)] {
        table.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        table.extend_from_slice(&(raw.len() as u32).to_le_bytes());
    }
    table.extend_from_slice(&2_u32.to_le_bytes());
    table.push(0);
    table.extend_from_slice(&0x8F92EAB1_u32.to_le_bytes());
    assert_eq!(table.len(), 33);
    pack.extend_from_slice(&table);

    let digest = sha256_bytes(&pack);
    client
        .put_object()
        .bucket(BUCKET)
        .key(object_key)
        .content_type("application/zstd")
        .metadata("sha256", hex::encode(digest))
        .if_none_match("*")
        .body(ByteStream::from(pack.clone()))
        .send()
        .await
        .unwrap();

    // Real S3 object must remain private without any signed whole-PACK URL.
    let anonymous = format!("{endpoint}/{BUCKET}/{object_key}");
    let curl = Command::new("curl")
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
    assert!(curl.status.success());
    assert_eq!(String::from_utf8(curl.stdout).unwrap(), "403");

    let neighbor = FreeChapterObject::for_markdown(8161, 9161, earlier).unwrap();
    let edition = FreeChapterObject::for_markdown(8162, 9162, entitled).unwrap();
    let manifest = serde_json::json!({
        "format": "tor-store-public-seekable-zstd",
        "manifest_version": 1,
        "zstd_seekable_version": "0.1.0",
        "pack_sha256": hex::encode(digest),
        "pack_encoded_bytes": pack.len(),
        "seek_table_offset": seek_start,
        "seek_table_bytes": table.len(),
        "frames": 2,
        "entries": [
            {
                "edition_id": "8161:9161",
                "frame": 0,
                "offset": 0,
                "encoded_bytes": first.len(),
                "raw_bytes": earlier.len(),
                "sha256": hex::encode(neighbor.sha256()),
            },
            {
                "edition_id": "8162:9162",
                "frame": 1,
                "offset": first.len(),
                "encoded_bytes": second.len(),
                "raw_bytes": entitled.len(),
                "sha256": hex::encode(edition.sha256()),
            }
        ],
    });
    let sidecar = serde_json::to_vec(&manifest).unwrap();
    let manifest_key = object_key.replace(".zst", ".manifest.json");
    client
        .put_object()
        .bucket(BUCKET)
        .key(&manifest_key)
        .content_type("application/json")
        .if_none_match("*")
        .body(ByteStream::from(sidecar.clone()))
        .send()
        .await
        .unwrap();
    let target = PackProofTarget {
        bucket: BUCKET.to_owned(),
        pack_key: object_key.to_owned(),
        manifest_key,
        pack_sha256: digest,
        manifest_sha256: sha256_bytes(&sidecar),
        pack_bytes: pack.len() as u64,
        manifest_bytes: sidecar.len() as u64,
        seek_table_start: seek_start as u64,
    };
    let verifier = S3PackProofStore::new(client.clone());
    let proven = verifier
        .verify_independent_full_get(&target, &[neighbor.clone(), edition.clone()])
        .await
        .unwrap();
    assert_eq!(proven.frames().len(), 2);
    assert_eq!(proven.frames()[0].edition(), &neighbor);
    assert_eq!(proven.frames()[1].edition(), &edition);
    let frame = proven.frames()[1].clone();
    assert_eq!(
        verifier
            .verify_independent_full_get(&target, std::slice::from_ref(&edition))
            .await
            .err(),
        Some(StorageError::Corrupt),
        "incomplete B inventory cannot certify a multi-edition PACK"
    );
    assert_eq!(
        frame.gateway_range(),
        format!("bytes={}-{}", first.len(), seek_start - 1)
    );

    assert!(response_metadata_matches(
        &frame,
        Some(format!("bytes {}-{}/{}", first.len(), seek_start - 1, pack.len()).as_str()),
        Some(second.len() as i64),
        None
    ));
    let plan = plan_after_acl(&edition, FreePhysicalPlacement::Pack(frame.clone())).unwrap();
    let store = S3PackFrameStore::new(client.clone());
    let mut calls = 0;
    let retrieved = read_reauthorized_pack(
        || {
            calls += 1;
            ready(Ok(Some(plan.clone())))
        },
        &store,
    )
    .await
    .unwrap();
    assert_eq!(retrieved, entitled.as_bytes());
    assert_eq!(calls, 2);
    assert!(!String::from_utf8_lossy(&retrieved).contains(earlier));

    let mut approvals = std::collections::VecDeque::from([Ok(Some(plan)), Ok(None)]);
    assert_eq!(
        read_reauthorized_pack(|| ready(approvals.pop_front().unwrap()), &store)
            .await
            .err(),
        Some(StorageError::Invalid),
        "an authorization revocation after S3 read must not release plaintext"
    );

    // The source frame must pass strict SHA256 even if a test-only admin
    // overwrites the immutable object, bypassing normal conditional PUT.
    client
        .put_object()
        .bucket(BUCKET)
        .key(object_key)
        .body(ByteStream::from(vec![0x47; pack.len()]))
        .send()
        .await
        .unwrap();
    let plan = plan_after_acl(&edition, FreePhysicalPlacement::Pack(frame)).unwrap();
    assert!(
        read_reauthorized_pack(|| ready(Ok(Some(plan.clone()))), &store)
            .await
            .is_err(),
        "tampered storage bytes must never become a valid edition"
    );
    assert_eq!(
        verifier
            .verify_independent_full_get(&target, &[neighbor, edition])
            .await
            .err(),
        Some(StorageError::Corrupt),
        "full object readback must detect corruption before any PG receipt"
    );
}
