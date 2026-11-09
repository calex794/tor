//! G/S349: immutable, private, unencrypted Markdown objects.
//! Only this free-chapter namespace is allowed; paid ciphertext is unaffected.
use async_trait::async_trait;
use aws_sdk_s3::{presigning::PresigningConfig, primitives::ByteStream, Client};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tor_contracts::paid_delivery::{validate_signed_get_ttl, SecretValue};

use crate::{
    domain::provider::{valid_private_bucket, StorageError},
    services::{
        crypto::sha256_bytes,
        free_chapter::{FreeChapterObject, FreeChapterStore, FREE_MARKDOWN_CONTENT_TYPE},
    },
};

#[derive(Clone)]
pub struct S3FreeChapterStore {
    client: Client,
    bucket: String,
}

impl S3FreeChapterStore {
    pub fn new(client: Client, private_bucket: String) -> Result<Self, StorageError> {
        if !valid_private_bucket(&private_bucket) {
            return Err(StorageError::Invalid);
        }
        Ok(Self {
            client,
            bucket: private_bucket,
        })
    }

    fn metadata_matches(
        object: &FreeChapterObject,
        length: Option<i64>,
        content_type: Option<&str>,
        content_encoding: Option<&str>,
        digest: Option<&str>,
    ) -> bool {
        length == Some(object.size_bytes() as i64)
            && content_type == Some(FREE_MARKDOWN_CONTENT_TYPE)
            && content_encoding.is_none()
            && digest == Some(hex::encode(object.sha256()).as_str())
    }

    /// HEAD is a cheap preflight. Published content must first pass a full GET.
    pub async fn head_matches(&self, object: &FreeChapterObject) -> Result<bool, StorageError> {
        let head = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(object.key())
            .send()
            .await
            .map_err(|_| StorageError::Unavailable)?;
        Ok(Self::metadata_matches(
            object,
            head.content_length(),
            head.content_type(),
            head.content_encoding(),
            head.metadata()
                .and_then(|m| m.get("sha256"))
                .map(String::as_str),
        ))
    }
}

#[async_trait]
impl FreeChapterStore for S3FreeChapterStore {
    async fn put_immutable(
        &self,
        object: &FreeChapterObject,
        bytes: &[u8],
    ) -> Result<(), StorageError> {
        if bytes.len() as u64 != object.size_bytes()
            || sha256_bytes(bytes) != *object.sha256()
            || std::str::from_utf8(bytes).is_err()
        {
            return Err(StorageError::Invalid);
        }
        // Reject overwriting an earlier edition, even after a client timeout.
        // A repeated identical PUT is handled by a verified GET in G service.
        let result = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(object.key())
            .content_type(FREE_MARKDOWN_CONTENT_TYPE)
            .cache_control("private, no-store")
            .metadata("sha256", hex::encode(object.sha256()))
            .if_none_match("*")
            .body(ByteStream::from(bytes.to_vec()))
            .send()
            .await;
        match result {
            Ok(_) => Ok(()),
            Err(error) => {
                use aws_sdk_s3::error::ProvideErrorMetadata;
                let conflict =
                    error
                        .as_service_error()
                        .and_then(|s| s.code())
                        .is_some_and(|code| {
                            matches!(code, "PreconditionFailed" | "ConditionalRequestConflict")
                        });
                Err(if conflict {
                    StorageError::Conflict
                } else {
                    StorageError::Unavailable
                })
            }
        }
    }

    async fn get_verified(&self, object: &FreeChapterObject) -> Result<Vec<u8>, StorageError> {
        let response = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(object.key())
            .send()
            .await
            .map_err(|_| StorageError::Unavailable)?;
        if !Self::metadata_matches(
            object,
            response.content_length(),
            response.content_type(),
            response.content_encoding(),
            response
                .metadata()
                .and_then(|m| m.get("sha256"))
                .map(String::as_str),
        ) {
            return Err(StorageError::Corrupt);
        }
        // Even a lying Content-Length cannot make this reader allocate >200 KB.
        let mut reader = response
            .body
            .into_async_read()
            .take(object.size_bytes() + 1);
        let mut bytes = Vec::with_capacity((object.size_bytes() as usize).min(65_536));
        reader
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| StorageError::Unavailable)?;
        if bytes.len() as u64 != object.size_bytes()
            || sha256_bytes(&bytes) != *object.sha256()
            || std::str::from_utf8(&bytes).is_err()
        {
            return Err(StorageError::Corrupt);
        }
        Ok(bytes)
    }

    async fn presign_verified(
        &self,
        object: &FreeChapterObject,
        ttl: Duration,
    ) -> Result<SecretValue, StorageError> {
        if validate_signed_get_ttl(ttl).is_err() {
            return Err(StorageError::Invalid);
        }
        if !self.head_matches(object).await? {
            return Err(StorageError::Corrupt);
        }
        // Caller must recheck B/C age, publication and moderation on EACH
        // request. A signed URL is a short-lived bearer capability.
        let config = PresigningConfig::expires_in(ttl).map_err(|_| StorageError::Invalid)?;
        let signed = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(object.key())
            .presigned(config)
            .await
            .map_err(|_| StorageError::Unavailable)?;
        let url = signed.uri().to_string();
        if !url.starts_with("https://")
            && !url.starts_with("http://127.0.0.1:")
            && !url.starts_with("http://localhost:")
        {
            return Err(StorageError::Unavailable);
        }
        SecretValue::new(url).map_err(|_| StorageError::Unavailable)
    }
}
