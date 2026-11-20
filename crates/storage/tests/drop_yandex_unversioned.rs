//! FND-02: manual, read/write-only real-provider probe for never-versioned
//! Yandex buckets. No DELETE, GC, migration, production keys, or live routes.
#![cfg(feature = "s3")]

use aws_sdk_s3::{
    config::{retry::RetryConfig, timeout::TimeoutConfig, BehaviorVersion, Credentials, Region},
    primitives::ByteStream,
    Client,
};
use std::{
    env,
    process::Command,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::io::AsyncReadExt;
use tor_storage::{
    domain::provider::StorageError,
    repositories::{
        free_pack_proof_s3::S3PackProofStore, free_pack_range_s3::S3PackFrameStore,
        free_pack_write_s3::S3FreePackWriter,
    },
    services::{
        free_chapter::FreeChapterObject,
        free_pack_read::{decode_exact_frame, PackFrameStore},
        free_pack_write::prepare_free_pack,
    },
};

const ENDPOINT: &str = "https://storage.yandexcloud.net";
const RANGES: usize = 24;

fn disposable_bucket() -> String {
    assert_eq!(
        env::var("TOR_FND02_YANDEX_PROBE").as_deref(),
        Ok("READ_WRITE"),
        "explicit TOR_FND02_YANDEX_PROBE=READ_WRITE required"
    );
    let bucket = env::var("TOR_FND02_YANDEX_BUCKET").expect("separate disposable bucket required");
    assert!(
        bucket
            .strip_prefix("tor-fnd02-probe-")
            .is_some_and(|suffix| {
                (8..=40).contains(&suffix.len())
                    && suffix
                        .bytes()
                        .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
            }),
        "only tor-fnd02-probe-<8..40 lowercase alphanumerics> is allowed"
    );
    bucket
}

fn probe_client() -> Client {
    let access = env::var("TOR_FND02_YANDEX_ACCESS_KEY").expect("dedicated access key required");
    let secret = env::var("TOR_FND02_YANDEX_SECRET_KEY").expect("dedicated secret key required");
    Client::from_conf(
        aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("ru-central1"))
            .credentials_provider(Credentials::new(
                access,
                secret,
                None,
                None,
                "fnd02-isolated",
            ))
            .endpoint_url(ENDPOINT)
            .force_path_style(true)
            .retry_config(RetryConfig::standard().with_max_attempts(1))
            .timeout_config(
                TimeoutConfig::builder()
                    .operation_timeout(Duration::from_secs(15))
                    .build(),
            )
            .build(),
    )
}

fn new_key() -> String {
    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("valid system clock")
        .as_nanos();
    format!("private/free-packs/fnd02/run-{token}-immutable.zst")
}

async fn assert_exact_bytes(client: &Client, bucket: &str, key: &str, expected: &[u8]) {
    let head = client
        .head_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .unwrap_or_else(|_| panic!("full-object HEAD must succeed"));
    assert!(
        head.version_id().is_none_or(|version| version == "null"),
        "unversioned HEAD cannot rely on S3 VersionId"
    );
    assert_eq!(head.content_length(), Some(expected.len() as i64));
    assert!(head.content_encoding().is_none());

    let get = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await
        .unwrap_or_else(|_| panic!("full-object GET must succeed"));
    assert!(
        get.version_id().is_none_or(|version| version == "null"),
        "unversioned GET cannot rely on S3 VersionId"
    );
    assert_eq!(get.content_length(), Some(expected.len() as i64));
    assert!(get.content_encoding().is_none());
    let mut actual = Vec::with_capacity(expected.len());
    let read = get
        .body
        .into_async_read()
        .take(expected.len() as u64 + 1)
        .read_to_end(&mut actual)
        .await;
    assert!(read.is_ok(), "bounded full GET stream failed");
    assert_eq!(
        actual, expected,
        "provider changed immutable exact-key bytes"
    );
}

fn assert_anonymous_denied(bucket: &str, key: &str, range: Option<&str>) {
    let mut curl = Command::new("curl");
    curl.args([
        "--silent",
        "--max-time",
        "10",
        "--output",
        "/dev/null",
        "--write-out",
        "%{http_code}",
    ]);
    if let Some(value) = range {
        curl.arg("--header").arg(format!("Range: {value}"));
    }
    let response = curl
        .arg(format!("{ENDPOINT}/{bucket}/{key}"))
        .output()
        .expect("curl installed on approved operator host");
    assert!(
        response.status.success(),
        "anonymous probe transport failed"
    );
    assert_eq!(
        response.stdout, b"403",
        "anonymous object read must be denied"
    );
}

#[tokio::test]
#[ignore = "MANUAL only: dedicated new empty nonversioned private Yandex bucket"]
async fn fnd02_yandex_nonversioned_pack_put_replay_full_and_range_acceptance() {
    let bucket = disposable_bucket();
    let client = probe_client();

    // Never create or modify bucket versioning. A suspended bucket is not a
    // never-versioned bucket, and an existing catalog must not be reused.
    let versioning = client
        .get_bucket_versioning()
        .bucket(&bucket)
        .send()
        .await
        .unwrap_or_else(|_| panic!("read-only bucket versioning inspection failed"));
    assert!(
        versioning.status().is_none(),
        "FND-02 requires a never-versioned private disposable bucket"
    );
    let listing = client
        .list_objects_v2()
        .bucket(&bucket)
        .max_keys(1)
        .send()
        .await
        .unwrap_or_else(|_| panic!("read-only empty-bucket inspection failed"));
    assert!(listing.contents().is_empty(), "probe bucket must be empty");

    let pack_key = new_key();
    // A fresh non-conditional write under this test-only prefix must fail
    // under the dedicated bucket policy. If the policy is absent the test
    // leaves a harmless synthetic orphan and fails before writing the PACK.
    let unguarded_key = pack_key.replace("-immutable.zst", "-policy.zst");
    let unguarded = client
        .put_object()
        .bucket(&bucket)
        .key(&unguarded_key)
        .body(ByteStream::from_static(b"synthetic policy-only fixture"))
        .send()
        .await;
    assert!(
        unguarded.err().is_some_and(|error| {
            error
                .raw_response()
                .is_some_and(|response| response.status().as_u16() == 403)
        }),
        "bucket policy must deny any PUT missing If-None-Match"
    );

    let sources: Vec<_> = [
        "Synthetic FREE chapter one 中文",
        "Synthetic FREE chapter two 😀",
    ]
    .into_iter()
    .enumerate()
    .map(|(index, text)| {
        (
            FreeChapterObject::for_markdown(index as i64 + 30_001, index as i64 + 40_001, text)
                .expect("synthetic edition"),
            text.as_bytes().to_vec(),
        )
    })
    .collect();
    let prepared =
        prepare_free_pack(&bucket, &pack_key, &sources).expect("valid synthetic seekable PACK");
    let writer = S3FreePackWriter::new(client.clone());
    let started = Instant::now();
    let initial = writer
        .upload_and_verify(&prepared)
        .await
        .unwrap_or_else(|_| panic!("conditional PUT and full-pack proof failed"));
    assert_eq!(initial.placement_receipt(), *prepared.receipt());
    let first_ms = started.elapsed().as_millis();

    for (key, bytes) in [
        (&prepared.target().pack_key, prepared.pack_bytes()),
        (&prepared.target().manifest_key, prepared.manifest_bytes()),
    ] {
        assert_exact_bytes(&client, &bucket, key, bytes).await;
        assert_anonymous_denied(&bucket, key, None);
    }

    let identical = writer
        .upload_and_verify(&prepared)
        .await
        .unwrap_or_else(|_| panic!("identical conditional replay must reconcile by full GET"));
    assert_eq!(identical.placement_receipt(), *prepared.receipt());
    let divergent_put = client
        .put_object()
        .bucket(&bucket)
        .key(&pack_key)
        .if_none_match("*")
        .body(ByteStream::from_static(b"synthetic mismatching PACK"))
        .send()
        .await;
    assert!(
        divergent_put.err().is_some_and(|error| {
            error
                .raw_response()
                .is_some_and(|response| response.status().as_u16() == 412)
        }),
        "altered conditional PUT must be rejected with HTTP 412"
    );
    let forged_source = "Synthetic tampering with chapter one";
    let forged = prepare_free_pack(
        &bucket,
        &pack_key,
        &[
            (
                FreeChapterObject::for_markdown(30_001, 40_001, forged_source)
                    .expect("synthetic changed edition"),
                forged_source.as_bytes().to_vec(),
            ),
            sources[1].clone(),
        ],
    )
    .expect("valid divergent synthetic PACK");
    assert_ne!(forged.target().pack_sha256, prepared.target().pack_sha256);
    assert_eq!(
        writer.upload_and_verify(&forged).await.err(),
        Some(StorageError::Corrupt),
        "digest-divergent retry must never register a valid PACK"
    );
    S3PackProofStore::new(client.clone())
        .verify_independent_full_get(prepared.target(), prepared.editions())
        .await
        .unwrap_or_else(|_| panic!("original PACK and manifest must remain intact"));

    let frame_store = S3PackFrameStore::new(client.clone());
    let mut latencies = Vec::with_capacity(RANGES);
    let mut returned_bytes = 0usize;
    for index in 0..RANGES {
        let member = index % sources.len();
        let frame = &initial.frames()[member];
        let start = Instant::now();
        let encoded = frame_store
            .fetch_frame(frame)
            .await
            .unwrap_or_else(|_| panic!("exact private Range GET failed"));
        returned_bytes += encoded.len();
        let decoded =
            decode_exact_frame(frame, &encoded).expect("strict Range frame decompression failed");
        assert_eq!(decoded, sources[member].1);
        latencies.push(start.elapsed().as_micros());
        if index == 0 {
            assert_anonymous_denied(&bucket, &pack_key, Some(&frame.gateway_range()));
        }
    }
    latencies.sort_unstable();
    println!(
        "FND02_YANDEX_NONVERSIONED_READ_PASS full_objects=2 ranges={} \
         upload_full_proof_ms={} range_p50_us={} range_p95_us={} range_bytes={}",
        RANGES,
        first_ms,
        latencies[RANGES.div_ceil(2) - 1],
        latencies[(RANGES * 95).div_ceil(100) - 1],
        returned_bytes
    );
}
