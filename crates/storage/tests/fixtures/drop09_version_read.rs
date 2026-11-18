//! Shared version-addressed read checks for the provider and MinIO probes.
use aws_sdk_s3::{primitives::ByteStream, Client};
use tokio::io::AsyncReadExt;
use tor_storage::{
    repositories::free_pack_range_s3::response_metadata_matches,
    services::{
        free_pack_write::PreparedFreePack,
        free_placement::{PackMetadata, ValidatedPackReference},
    },
};

pub fn pack_frame(pack: &PreparedFreePack, member: usize) -> ValidatedPackReference {
    let target = pack.target();
    let frame = &pack.receipt().frames[member];
    ValidatedPackReference::new(
        pack.editions()[member].clone(),
        PackMetadata {
            bucket: target.bucket.clone(),
            key: target.pack_key.clone(),
            pack_bytes: target.pack_bytes,
            seek_table_start: target.seek_table_start,
            frame_offset: frame.frame_offset,
            frame_bytes: frame.frame_bytes,
            pack_sha256: target.pack_sha256,
            manifest_sha256: target.manifest_sha256,
        },
    )
    .unwrap()
}

pub async fn current_version(client: &Client, bucket: &str, key: &str) -> String {
    let head = client.head_object().bucket(bucket).key(key).send().await;
    let Ok(head) = head else {
        panic!("current object HEAD failed");
    };
    head.version_id()
        .filter(|id| !id.is_empty() && *id != "null")
        .expect("object must have a non-null version")
        .to_owned()
}

pub async fn assert_exact_object(
    client: &Client,
    bucket: &str,
    key: &str,
    version: &str,
    expected: &[u8],
) {
    let head = client
        .head_object()
        .bucket(bucket)
        .key(key)
        .version_id(version)
        .send()
        .await;
    let Ok(head) = head else {
        panic!("version-specific HEAD failed");
    };
    assert!(
        head.version_id() == Some(version) && head.content_length() == Some(expected.len() as i64),
        "HEAD returned the wrong version or length"
    );
    let get = client
        .get_object()
        .bucket(bucket)
        .key(key)
        .version_id(version)
        .send()
        .await;
    let Ok(get) = get else {
        panic!("version-specific full GET failed");
    };
    assert!(
        get.version_id() == Some(version)
            && get.content_length() == Some(expected.len() as i64)
            && get.content_encoding().is_none(),
        "full GET returned incorrect version metadata"
    );
    assert!(
        bounded_bytes(get.body, expected.len() as u64).await == expected,
        "version-specific full GET returned different bytes"
    );
}

pub async fn exact_version_range(
    client: &Client,
    frame: &ValidatedPackReference,
    version: &str,
) -> Vec<u8> {
    let meta = frame.metadata();
    let response = client
        .get_object()
        .bucket(&meta.bucket)
        .key(&meta.key)
        .version_id(version)
        .range(frame.gateway_range())
        .send()
        .await;
    let Ok(response) = response else {
        panic!("version-specific Range GET failed");
    };
    assert!(
        response.version_id() == Some(version)
            && response_metadata_matches(
                frame,
                response.content_range(),
                response.content_length(),
                response.content_encoding(),
            ),
        "Range GET returned incorrect version or range metadata"
    );
    let bytes = bounded_bytes(response.body, meta.frame_bytes).await;
    assert_eq!(bytes.len() as u64, meta.frame_bytes);
    bytes
}

async fn bounded_bytes(body: ByteStream, expected: u64) -> Vec<u8> {
    let mut bytes = Vec::new();
    assert!(
        body.into_async_read()
            .take(expected + 1)
            .read_to_end(&mut bytes)
            .await
            .is_ok(),
        "version-specific response body failed"
    );
    bytes
}
