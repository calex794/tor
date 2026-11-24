//! DROP-R3a: server-side-only private S3 Range GET for an authorized PACK frame.
//!
//! Never issue a presigned GET for this object. Range offsets must come from
//! B's trusted immutable edition placement, not client headers or JSON. This
//! provider proves the HTTP reply is the exact requested frame, then G verifies
//! Zstd/decoded SHA256 and C rechecks rights before any response is exposed.

use async_trait::async_trait;
use aws_sdk_s3::Client;

use crate::{
    domain::provider::StorageError,
    services::{
        bounded_body::read_bounded_body, free_pack_read::PackFrameStore,
        free_placement::ValidatedPackReference,
    },
};

#[derive(Clone)]
pub struct S3PackFrameStore {
    client: Client,
}

impl S3PackFrameStore {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

/// Confirm the provider actually honored the Range and returned the whole
/// expected compressed frame, not a full PACK (200), short payload, invented
/// Content-Range, altered total length, or transparent content encoding.
pub fn response_metadata_matches(
    frame: &ValidatedPackReference,
    content_range: Option<&str>,
    content_length: Option<i64>,
    content_encoding: Option<&str>,
) -> bool {
    let m = frame.metadata();
    let last = m.frame_offset + m.frame_bytes - 1;
    let expected = format!("bytes {}-{}/{}", m.frame_offset, last, m.pack_bytes);
    content_range == Some(expected.as_str())
        && content_length == Some(m.frame_bytes as i64)
        && content_encoding.is_none()
}

#[async_trait]
impl PackFrameStore for S3PackFrameStore {
    async fn fetch_frame(&self, frame: &ValidatedPackReference) -> Result<Vec<u8>, StorageError> {
        let m = frame.metadata();
        let response = self
            .client
            .get_object()
            .bucket(&m.bucket)
            .key(&m.key)
            .range(frame.gateway_range())
            .send()
            .await
            .map_err(|_| StorageError::Unavailable)?;

        if !response_metadata_matches(
            frame,
            response.content_range(),
            response.content_length(),
            response.content_encoding(),
        ) {
            return Err(StorageError::Corrupt);
        }

        // A PACK-wide SHA-256 in S3 user metadata is a useful guard but NOT
        // independent evidence of immutable PACK content. The B/G publisher
        // must have verified the whole object and sidecar manifest beforehand.
        if response
            .metadata()
            .and_then(|meta| meta.get("sha256"))
            .is_some_and(|digest| digest != &hex::encode(m.pack_sha256))
        {
            return Err(StorageError::Corrupt);
        }

        let encoded = read_bounded_body(response.body.into_async_read(), m.frame_bytes).await?;
        if encoded.len() as u64 != m.frame_bytes {
            return Err(StorageError::Corrupt);
        }
        Ok(encoded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{
        free_chapter::FreeChapterObject,
        free_placement::{PackMetadata, ValidatedPackReference},
    };

    fn frame() -> ValidatedPackReference {
        let edition = FreeChapterObject::for_markdown(20, 30, "Hello pack!").unwrap();
        let pack = PackMetadata {
            bucket: "tor-private-range-ci".into(),
            key: "private/free-packs/ci/proof-r3a.zst".into(),
            pack_bytes: 1000,
            seek_table_start: 950,
            frame_offset: 200,
            frame_bytes: 100,
            pack_sha256: [4; 32],
            manifest_sha256: [5; 32],
        };
        ValidatedPackReference::new(edition, pack).unwrap()
    }

    #[test]
    fn range_headers_must_be_exact_never_silently_accept_full_pack() {
        let r = frame();
        assert!(response_metadata_matches(
            &r,
            Some("bytes 200-299/1000"),
            Some(100),
            None
        ));
        for (range, size, encoding) in [
            (Some("bytes 0-999/1000"), Some(1000), None),
            (Some("bytes 200-299/999"), Some(100), None),
            (Some("bytes 200-300/1000"), Some(101), None),
            (Some("bytes 200-299/1000"), Some(99), None),
            (Some("bytes 200-299/1000"), Some(100), Some("gzip")),
            (None, Some(100), None),
        ] {
            assert!(
                !response_metadata_matches(&r, range, size, encoding),
                "bad partial response must fail"
            );
        }
    }
}
