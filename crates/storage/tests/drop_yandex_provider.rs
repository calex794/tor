//! DROP-09a: explicit synthetic provider evidence in a dedicated test bucket.
#![cfg(feature = "s3")]

use aws_sdk_s3::{
    config::{retry::RetryConfig, timeout::TimeoutConfig, BehaviorVersion, Credentials, Region},
    error::ProvideErrorMetadata,
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

fn probe_bucket(mode: &str) -> String {
    assert!(
        env::var("TOR_DROP_PROVIDER_PROBE").as_deref() == Ok(mode),
        "set TOR_DROP_PROVIDER_PROBE={mode} to run this probe"
    );
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
    bucket
}

fn probe_client(access_key: String, secret_key_var: &str) -> Client {
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("ru-central1"))
        .credentials_provider(Credentials::new(
            access_key,
            env::var(secret_key_var).expect("probe secret key required"),
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
    Client::from_conf(config)
}

fn fixture_key(suffix: &str) -> String {
    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("private/free-packs/provider-probe/run-{token}-{suffix}.zst")
}

#[tokio::test]
#[ignore = "requires explicit operator intent and a dedicated private Yandex test bucket"]
async fn drop_09_yandex_private_pack_read_write_acceptance() {
    let bucket = probe_bucket("READ_WRITE");
    let client = probe_client(
        env::var("TOR_DROP_PROVIDER_ACCESS_KEY").expect("probe access key required"),
        "TOR_DROP_PROVIDER_SECRET_KEY",
    );
    let key = fixture_key("read-write");
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
    let target = prepared.target();
    for object_key in [&target.pack_key, &target.manifest_key] {
        let stored = client
            .head_object()
            .bucket(&bucket)
            .key(object_key)
            .send()
            .await
            .expect("versioned probe object metadata");
        assert!(
            stored
                .version_id()
                .is_some_and(|version| !version.is_empty() && version != "null"),
            "PACK and manifest must have non-null version IDs"
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
                &format!("{ENDPOINT}/{bucket}/{object_key}"),
            ])
            .output()
            .expect("curl required for the anonymous request");
        assert!(anonymous.status.success());
        assert_eq!(anonymous.stdout, b"403", "anonymous read must be denied");
    }

    let store = S3PackFrameStore::new(client.clone());
    let mut latencies = Vec::with_capacity(SAMPLES);
    let mut range_bytes = 0;
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
}

#[tokio::test]
#[ignore = "uses DELETE on explicitly opted-in synthetic versions in a private Yandex test bucket"]
async fn drop_09_yandex_exact_version_delete_acceptance() {
    let bucket = probe_bucket("DELETE");
    let writer_key = env::var("TOR_DROP_PROVIDER_ACCESS_KEY").expect("probe access key required");
    let delete_key = env::var("TOR_DROP_PROVIDER_DELETE_ACCESS_KEY")
        .expect("separate delete access key required");
    assert!(
        writer_key != delete_key,
        "read/write and retirement must use separate credentials"
    );
    let writer = probe_client(writer_key, "TOR_DROP_PROVIDER_SECRET_KEY");
    let deleter = probe_client(delete_key, "TOR_DROP_PROVIDER_DELETE_SECRET_KEY");
    probe_version_deletion(&writer, &deleter, &bucket, &fixture_key("delete")).await;
}

async fn fixture_present(
    client: &Client,
    bucket: &str,
    key: &str,
    version: Option<&str>,
    expected_version: &str,
    fixture: &[u8],
) -> bool {
    let result = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .set_version_id(version.map(str::to_owned))
        .send()
        .await;
    let Ok(object) = result else {
        return false;
    };
    if object.version_id() != Some(expected_version)
        || object.content_length() != Some(fixture.len() as i64)
    {
        return false;
    }
    let mut bytes = Vec::with_capacity(fixture.len());
    let read = object
        .body
        .into_async_read()
        .take(fixture.len() as u64 + 1)
        .read_to_end(&mut bytes)
        .await;
    read.is_ok() && bytes == fixture
}

async fn probe_version_deletion(writer: &Client, deleter: &Client, bucket: &str, key: &str) {
    const OLD: &[u8] = b"synthetic old delete fixture";
    const NEW: &[u8] = b"synthetic newer delete fixture";
    // These writes and both DELETE attempts use one unique fixture key.
    let old = writer
        .put_object()
        .bucket(bucket)
        .key(key)
        .if_none_match("*")
        .body(ByteStream::from_static(OLD))
        .send()
        .await
        .expect("first delete probe fixture upload");
    let Some((version, etag)) = old.version_id().zip(old.e_tag()) else {
        panic!("DROP09_PROVIDER_DELETE_PROBE accepted=false reason=missing_version_or_etag");
    };
    if version.is_empty() || version == "null" || etag.is_empty() {
        panic!("DROP09_PROVIDER_DELETE_PROBE accepted=false reason=missing_version_or_etag");
    }
    let new = writer
        .put_object()
        .bucket(bucket)
        .key(key)
        .if_match(etag)
        .body(ByteStream::from_static(NEW))
        .send()
        .await
        .expect("second delete probe fixture upload");
    let Some(new_version) = new
        .version_id()
        .filter(|new| !new.is_empty() && *new != "null" && *new != version)
    else {
        panic!("DROP09_PROVIDER_DELETE_PROBE accepted=false reason=no_distinct_newer_version");
    };
    let writer_delete = writer
        .delete_object()
        .bucket(bucket)
        .key(key)
        .version_id(version)
        .if_match(etag)
        .send()
        .await;
    assert!(
        writer_delete.err().is_some_and(|error| {
            error.as_service_error().and_then(|e| e.code()) == Some("AccessDenied")
                && error
                    .raw_response()
                    .is_some_and(|r| r.status().as_u16() == 403)
        }) && fixture_present(writer, bucket, key, Some(version), version, OLD).await,
        "read/write identity must receive AccessDenied and preserve the old version"
    );
    let result = deleter
        .delete_object()
        .bucket(bucket)
        .key(key)
        .version_id(version)
        .if_match("\"00000000000000000000000000000000\"")
        .send()
        .await;
    let wrong_etag_rejected = result
        .err()
        .is_some_and(|e| e.raw_response().is_some_and(|r| r.status().as_u16() == 412));
    let old_preserved = fixture_present(writer, bucket, key, Some(version), version, OLD).await;
    let (mut old_removed, mut newer_preserved) = (false, false);
    if wrong_etag_rejected && old_preserved {
        let deleted = deleter
            .delete_object()
            .bucket(bucket)
            .key(key)
            .version_id(version)
            .if_match(etag)
            .send()
            .await;
        let absence = writer
            .get_object()
            .bucket(bucket)
            .key(key)
            .version_id(version)
            .send()
            .await;
        old_removed = deleted.is_ok()
            && absence.err().is_some_and(|e| {
                e.as_service_error().and_then(|e| e.code()) == Some("NoSuchVersion")
                    && e.raw_response().is_some_and(|r| r.status().as_u16() == 404)
            });
        newer_preserved = fixture_present(writer, bucket, key, None, new_version, NEW).await;
    }
    let accepted = wrong_etag_rejected && old_preserved && old_removed && newer_preserved;
    println!(
        "DROP09_PROVIDER_DELETE_PROBE accepted={accepted} writer_delete_denied=true \
         wrong_etag_412={wrong_etag_rejected} old_version_preserved={old_preserved} \
         exact_old_version_removed={old_removed} \
         current_newer_version_preserved={newer_preserved}"
    );
    assert!(
        accepted,
        "real provider exact-version deletion acceptance failed"
    );
}
