//! DROP-07a: read-only, bounded actual-byte probe for expected FREE objects.
//! No LIST, PUT, DELETE, signed URL, automatic repair or GC authorization.
//! A failed GET (including an ambiguous 404) is UNKNOWN, never a deletion cue.
use aws_sdk_s3::Client;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

use crate::domain::provider::{valid_private_bucket, StorageError};

const MAX_AUDIT_BYTES: u64 = 16_777_216;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FreeObjectEvidence {
    Verified,
    SizeMismatch,
    HashMismatch,
}

pub struct S3FreeExpectedObjectVerifier {
    client: Client,
}

fn valid_key(value: &str) -> bool {
    let free_single = value.starts_with("private/free-chapters/") && value.ends_with(".md");
    let free_pack = value.starts_with("private/free-packs/")
        && (value.ends_with(".zst") || value.ends_with(".manifest.json"));
    (free_single || free_pack)
        && (25..=260).contains(&value.len())
        && !value.contains("//")
        && !value
            .split('/')
            .any(|segment| segment == ".." || segment == ".")
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.'))
}

pub fn compare_actual_bytes(
    actual: &[u8],
    expected_sha256: &[u8; 32],
    expected_size: u64,
) -> FreeObjectEvidence {
    if actual.len() as u64 != expected_size {
        return FreeObjectEvidence::SizeMismatch;
    }
    let observed: [u8; 32] = Sha256::digest(actual).into();
    if &observed != expected_sha256 {
        return FreeObjectEvidence::HashMismatch;
    }
    FreeObjectEvidence::Verified
}

impl S3FreeExpectedObjectVerifier {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// A bounded, full actual-byte read. Cannot prove "missing" without
    /// an independent, authoritative 404 check; ambiguous errors never
    /// become an orphan or deletion candidate.
    pub async fn verify_exact(
        &self,
        bucket: &str,
        object_key: &str,
        expected_sha256: [u8; 32],
        expected_size: u64,
    ) -> Result<FreeObjectEvidence, StorageError> {
        if !valid_private_bucket(bucket)
            || !valid_key(object_key)
            || !(1..=MAX_AUDIT_BYTES).contains(&expected_size)
            || expected_sha256 == [0; 32]
        {
            return Err(StorageError::Invalid);
        }
        let response = self
            .client
            .get_object()
            .bucket(bucket)
            .key(object_key)
            .send()
            .await
            .map_err(|_| StorageError::Unavailable)?;
        if response.content_encoding().is_some() {
            return Err(StorageError::Corrupt);
        }
        if response.content_length() != Some(expected_size as i64) {
            return Ok(FreeObjectEvidence::SizeMismatch);
        }
        let mut reader = response.body.into_async_read().take(expected_size + 1);
        let mut body = Vec::with_capacity((expected_size as usize).min(65_536));
        reader
            .read_to_end(&mut body)
            .await
            .map_err(|_| StorageError::Unavailable)?;
        Ok(compare_actual_bytes(&body, &expected_sha256, expected_size))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_object_evidence_requires_exact_size_and_sha() {
        let expected: [u8; 32] = Sha256::digest(b"FREE text UTF-8").into();
        assert_eq!(
            compare_actual_bytes(b"FREE text UTF-8", &expected, 15),
            FreeObjectEvidence::Verified
        );
        assert_eq!(
            compare_actual_bytes(b"FREE text", &expected, 15),
            FreeObjectEvidence::SizeMismatch
        );
        assert_eq!(
            compare_actual_bytes(b"FREE wrong body", &expected, 15),
            FreeObjectEvidence::HashMismatch
        );
    }

    #[test]
    fn only_exact_private_free_object_names_are_probeable() {
        assert!(valid_key(
            "private/free-packs/import/abc1234567890.manifest.json"
        ));
        assert!(valid_key("private/free-chapters/123456789/123456789.md"));
        assert!(valid_key("private/free-chapters/1/1.md"));
        for name in [
            "public/free-packs/import/1234567890.zst",
            "private/free-chapters/../123456789.md",
            "private/free-packs//1234567890.zst",
            "private/free-packs/foo.exe",
            "private/paid/1234567890.enc",
        ] {
            assert!(!valid_key(name), "{name}");
        }
    }
}
