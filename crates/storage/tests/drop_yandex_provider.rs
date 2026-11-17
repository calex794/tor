//! DROP-09a: explicit synthetic provider evidence in a dedicated test bucket.
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
use tor_storage::{
    repositories::{free_pack_range_s3::S3PackFrameStore, free_pack_write_s3::S3FreePackWriter},
    services::{
        free_chapter::FreeChapterObject,
        free_pack_read::{decode_exact_frame, PackFrameStore},
        free_pack_write::prepare_free_pack,
        free_placement::{PackMetadata, ValidatedPackReference},
    },
};

const ENDPOINT: &str = "https://storage.yandexcloud.net";
const SAMPLES: usize = 24;

#[tokio::test]
#[ignore = "requires explicit operator intent and a dedicated private Yandex test bucket"]
async fn drop_09_yandex_private_pack_provider_acceptance() {
    assert_eq!(env::var("TOR_DROP_PROVIDER_PROBE").as_deref(), Ok("YES"));
    let bucket = env::var("TOR_DROP_PROVIDER_BUCKET").expect("disposable test bucket required");
    assert!(
        bucket
            .strip_prefix("tor-drop-probe-")
            .is_some_and(|suffix| {
                (8..=40).contains(&suffix.len())
                    && suffix
                        .bytes()
                        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            }),
        "bucket must use the dedicated tor-drop-probe- scope"
    );
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("ru-central1"))
        .credentials_provider(Credentials::new(
            env::var("TOR_DROP_PROVIDER_ACCESS_KEY").expect("probe credentials required"),
            env::var("TOR_DROP_PROVIDER_SECRET_KEY").expect("probe credentials required"),
            None,
            None,
            "drop09-isolated",
        ))
        .endpoint_url(ENDPOINT)
        .force_path_style(true)
        .retry_config(RetryConfig::standard().with_max_attempts(1))
        .timeout_config(
            TimeoutConfig::builder()
                .operation_timeout(Duration::from_secs(15))
                .build(),
        )
        .build();
    let client = Client::from_conf(config);
    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let key = format!("private/free-packs/provider-probe/run-{token}.zst");
    let sources: Vec<_> = ["Chapter one 😀\n\nsynthetic", "第二章\n\nsynthetic"]
        .into_iter()
        .enumerate()
        .map(|(i, text)| {
            (
                FreeChapterObject::for_markdown(i as i64 + 10, i as i64 + 20, text).unwrap(),
                text.as_bytes().to_vec(),
            )
        })
        .collect();
    let prepared = prepare_free_pack(&bucket, &key, &sources).unwrap();
    let start = Instant::now();
    assert!(
        S3FreePackWriter::new(client.clone())
            .upload_and_verify(&prepared)
            .await
            .is_ok(),
        "conditional PACK+manifest upload and independent full readback failed"
    );
    let upload_ms = start.elapsed().as_millis();
    let duplicate = client
        .put_object()
        .bucket(&bucket)
        .key(&key)
        .if_none_match("*")
        .body(ByteStream::from_static(b"synthetic rejected overwrite"))
        .send()
        .await;
    assert!(
        duplicate
            .err()
            .is_some_and(|e| { e.raw_response().is_some_and(|r| r.status().as_u16() == 412) }),
        "provider must reject a different conditional PUT for the same key"
    );
    let anonymous = Command::new("curl")
        .args([
            "--silent",
            "--max-time",
            "10",
            "--output",
            "/dev/null",
            "--write-out",
            "%{http_code}",
            &format!("{ENDPOINT}/{bucket}/{key}"),
        ])
        .output()
        .expect("curl required for the anonymous request");
    assert!(anonymous.status.success());
    assert_eq!(
        anonymous.stdout, b"403",
        "anonymous PACK read must be denied"
    );

    let store = S3PackFrameStore::new(client.clone());
    let mut latencies = Vec::with_capacity(SAMPLES);
    let mut range_bytes = 0;
    let target = prepared.target();
    for i in 0..SAMPLES {
        let member = i % sources.len();
        let frame = &prepared.receipt().frames[member];
        let reference = ValidatedPackReference::new(
            sources[member].0.clone(),
            PackMetadata {
                bucket: bucket.clone(),
                key: key.clone(),
                pack_bytes: target.pack_bytes,
                seek_table_start: target.seek_table_start,
                frame_offset: frame.frame_offset,
                frame_bytes: frame.frame_bytes,
                pack_sha256: target.pack_sha256,
                manifest_sha256: target.manifest_sha256,
            },
        )
        .unwrap();
        let start = Instant::now();
        let encoded = store
            .fetch_frame(&reference)
            .await
            .expect("exact provider Content-Range and length");
        let bytes = decode_exact_frame(&reference, &encoded).unwrap();
        assert_eq!(bytes, sources[member].1, "only designated frame");
        range_bytes += encoded.len();
        latencies.push(start.elapsed().as_micros());
    }
    let first = latencies[0];
    latencies.sort_unstable();
    println!(
        "DROP09_PROVIDER_READ_PASS records={} upload_and_full_proof_ms={} \
         first_range_us={} range_p50_us={} range_p95_us={} response_payload_bytes={}",
        SAMPLES,
        upload_ms,
        first,
        latencies[SAMPLES.div_ceil(2) - 1],
        latencies[(SAMPLES * 95).div_ceil(100) - 1],
        range_bytes + prepared.pack_bytes().len() + prepared.manifest_bytes().len()
    );

    // The documented Yandex DELETE API does not list an If-Match condition.
    // Probe only a newly written synthetic object in this run's own prefix.
    let delete_key = format!("private/free-packs/provider-probe/run-{token}-delete.zst");
    assert!(
        client
            .put_object()
            .bucket(&bucket)
            .key(&delete_key)
            .if_none_match("*")
            .body(ByteStream::from_static(b"synthetic delete test object"))
            .send()
            .await
            .is_ok(),
        "delete probe fixture upload failed"
    );
    let result = client
        .delete_object()
        .bucket(&bucket)
        .key(&delete_key)
        .if_match("\"00000000000000000000000000000000\"")
        .send()
        .await;
    let wrong_etag_rejected = result
        .err()
        .is_some_and(|e| e.raw_response().is_some_and(|r| r.status().as_u16() == 412));
    let head = client
        .head_object()
        .bucket(&bucket)
        .key(&delete_key)
        .send()
        .await;
    let preserved = head.is_ok();
    let (mut exact_etag_deleted, mut version_bound) = (false, false);
    if let (true, Ok(proof)) = (wrong_etag_rejected, head) {
        if let Some(etag) = proof.e_tag() {
            version_bound = proof.version_id().is_some_and(|version| version != "null");
            let deleted = client
                .delete_object()
                .bucket(&bucket)
                .key(&delete_key)
                .if_match(etag)
                .set_version_id(proof.version_id().map(str::to_owned))
                .send()
                .await;
            let absent = client
                .head_object()
                .bucket(&bucket)
                .key(&delete_key)
                .send()
                .await
                .err()
                .is_some_and(|e| e.raw_response().is_some_and(|r| r.status().as_u16() == 404));
            exact_etag_deleted = deleted.is_ok() && absent;
        }
    }
    println!(
        "DROP09_PROVIDER_DELETE_PROBE wrong_etag_412={} object_preserved={} \
         exact_etag_removed={} version_bound={}",
        wrong_etag_rejected, preserved, exact_etag_deleted, version_bound
    );
}
