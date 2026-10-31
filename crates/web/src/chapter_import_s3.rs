//! S023 private staging objects for owner-confirmed chapter imports.
#![cfg(feature = "s3-chapter-import")]
use aws_sdk_s3::{error::ProvideErrorMetadata, primitives::ByteStream, Client};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tor_imports::services::{
    ChapterImportObjectStore, ChapterImportStoreError as StoreError,
};

#[derive(Clone)]
pub struct S3ChapterImportStore {
    client: Client,
    bucket: String,
}

impl S3ChapterImportStore {
    pub fn new(client: Client, bucket: String) -> Result<Self, StoreError> {
        let valid = (3..=63).contains(&bucket.len())
            && bucket.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || byte == b'-'
                    || byte == b'.'
            })
            && !bucket.starts_with('.')
            && !bucket.ends_with('.')
            && !bucket.starts_with('-')
            && !bucket.ends_with('-')
            && !bucket.contains("..");
        if !valid {
            return Err(StoreError::Invalid);
        }
        Ok(Self { client, bucket })
    }

    fn valid_key(key: &str) -> bool {
        key.starts_with("staging/chapters/")
            && key.len() <= 512
            && !key.contains("..")
            && !key.contains("//")
            && key.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'/' | b'-' | b'_' | b'.')
            })
    }

    fn hex(hash: &[u8; 32]) -> String {
        hash.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    async fn verify_head(
        &self,
        key: &str,
        expected_size: i64,
        hash: &[u8; 32],
    ) -> Result<bool, StoreError> {
        let head = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|_| StoreError::Unavailable)?;
        Ok(head.content_length() == Some(expected_size)
            && head.content_type() == Some("application/octet-stream")
            && head
                .metadata()
                .and_then(|metadata| metadata.get("sha256"))
                .is_some_and(|value| value == &Self::hex(hash)))
    }
}

#[async_trait::async_trait]
impl ChapterImportObjectStore for S3ChapterImportStore {
    async fn put_verified(
        &self,
        key: &str,
        bytes: &[u8],
        sha256: &[u8; 32],
    ) -> Result<(), StoreError> {
        let actual: [u8; 32] = Sha256::digest(bytes).into();
        if !Self::valid_key(key)
            || bytes.is_empty()
            || bytes.len() > 700_000
            || actual != *sha256
        {
            return Err(StoreError::Invalid);
        }
        let digest = Self::hex(sha256);
        match self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(key)
            .content_type("application/octet-stream")
            .metadata("sha256", digest.as_str())
            .if_none_match("*")
            .body(ByteStream::from(bytes.to_vec()))
            .send()
            .await
        {
            Ok(_) => {}
            Err(error) => {
                let conflict = error
                    .as_service_error()
                    .and_then(|value| value.code())
                    .is_some_and(|code| {
                        matches!(code, "PreconditionFailed" | "ConditionalRequestConflict")
                    });
                if !conflict {
                    return Err(StoreError::Unavailable);
                }
                // A retry after an unknown PUT result is accepted only when
                // the immutable object already present is byte-for-byte equal.
                if !self
                    .verify_head(key, bytes.len() as i64, sha256)
                    .await?
                {
                    return Err(StoreError::Conflict);
                }
                let existing = self
                    .get_verified(key, bytes.len() as i64, sha256, 700_000)
                    .await?;
                return if existing == bytes {
                    Ok(())
                } else {
                    Err(StoreError::Corrupt)
                };
            }
        }
        if !self
            .verify_head(key, bytes.len() as i64, sha256)
            .await?
        {
            return Err(StoreError::Corrupt);
        }
        let stored = self
            .get_verified(key, bytes.len() as i64, sha256, 700_000)
            .await?;
        if stored != bytes {
            return Err(StoreError::Corrupt);
        }
        Ok(())
    }

    async fn get_verified(
        &self,
        key: &str,
        expected_size: i64,
        sha256: &[u8; 32],
        max_bytes: usize,
    ) -> Result<Vec<u8>, StoreError> {
        if !Self::valid_key(key)
            || expected_size <= 0
            || expected_size as usize > max_bytes
            || max_bytes > 700_000
        {
            return Err(StoreError::Invalid);
        }
        if !self.verify_head(key, expected_size, sha256).await? {
            return Err(StoreError::Corrupt);
        }
        let object = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|_| StoreError::Unavailable)?;
        if object.content_length() != Some(expected_size)
            || object.content_type() != Some("application/octet-stream")
            || object
                .metadata()
                .and_then(|metadata| metadata.get("sha256"))
                .is_none_or(|value| value != &Self::hex(sha256))
        {
            return Err(StoreError::Corrupt);
        }
        // Content-Length is not trusted: stop at max_bytes+1 while streaming.
        let mut reader = object.body.into_async_read().take(max_bytes as u64 + 1);
        let mut bytes = Vec::with_capacity(expected_size as usize);
        reader
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| StoreError::Unavailable)?;
        let actual: [u8; 32] = Sha256::digest(&bytes).into();
        if bytes.len() != expected_size as usize || actual != *sha256
        {
            return Err(StoreError::Corrupt);
        }
        Ok(bytes)
    }

    async fn delete(&self, key: &str) -> Result<(), StoreError> {
        if !Self::valid_key(key) {
            return Err(StoreError::Invalid);
        }
        self.client
            .delete_object()
            .bucket(&self.bucket)
            .key(key)
            .send()
            .await
            .map_err(|_| StoreError::Unavailable)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
