//! Private S3 storage for immutable ciphertext only. No plaintext, browser key
//! inputs, public buckets, automatic creation of buckets or anonymous fallback.
use async_trait::async_trait;
use aws_sdk_s3::{presigning::PresigningConfig, primitives::ByteStream, Client};
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tor_contracts::paid_delivery::{
    validate_signed_get_ttl, ObjectRef, SecretValue,
};

use crate::{
    domain::{
        object_keys::{PrivateObjectKey, ENCRYPTED_CONTENT_TYPE},
        provider::{valid_ciphertext_size, StorageError},
    },
    services::{crypto::sha256_bytes, provider::PrivateObjectStore},
};

#[derive(Clone)]
pub struct S3PrivateStore {
    client: Client,
    bucket: String,
}

impl S3PrivateStore {
    /// This fixed private bucket comes from trusted G configuration, never
    /// a user-supplied chapter request. Only legal DNS-compatible names.
    pub fn new(client: Client, private_bucket: String) -> Result<Self, StorageError> {
        let bytes = private_bucket.as_bytes();
        if !(3..=63).contains(&bytes.len())
            || !bytes[0].is_ascii_lowercase()
            || !bytes[bytes.len() - 1].is_ascii_alphanumeric()
            || !bytes.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
            || private_bucket.contains("--")
            || private_bucket.starts_with("public")
        {
            return Err(StorageError::Invalid);
        }
        Ok(Self {
            client,
            bucket: private_bucket,
        })
    }

    fn key(object: &ObjectRef) -> String {
        PrivateObjectKey::for_object(object.kind, &object.object_id)
            .as_str()
            .to_owned()
    }

    fn valid(object: &ObjectRef) -> bool {
        valid_ciphertext_size(object) && object.sha256_ciphertext != [0_u8; 32]
    }

    fn head_is_exact(
        object: &ObjectRef,
        size: Option<i64>,
        content_type: Option<&str>,
        content_encoding: Option<&str>,
        expected: Option<&str>,
    ) -> bool {
        size == Some(object.size_bytes as i64)
            && content_type == Some(ENCRYPTED_CONTENT_TYPE)
            && content_encoding.is_none()
            && expected == Some(hex::encode(object.sha256_ciphertext).as_str())
    }
}

#[async_trait]
impl PrivateObjectStore for S3PrivateStore {
    async fn put_immutable(
        &self,
        object: &ObjectRef,
        ciphertext: &[u8],
    ) -> Result<(), StorageError> {
        if !Self::valid(object)
            || ciphertext.len() as u64 != object.size_bytes
            || sha256_bytes(ciphertext) != object.sha256_ciphertext
        {
            return Err(StorageError::Invalid);
        }
        // The provider MUST honor If-None-Match: *. Never do check-then-put:
        // two writers could otherwise overwrite the same immutable edition.
        let written = self
            .client
            .put_object()
            .bucket(&self.bucket)
            .key(Self::key(object))
            .content_type(ENCRYPTED_CONTENT_TYPE)
            .metadata("sha256", hex::encode(object.sha256_ciphertext))
            .if_none_match("*")
            .body(ByteStream::from(ciphertext.to_vec()))
            .send()
            .await;
        match written {
            Ok(_) => {}
            Err(error) => {
                use aws_sdk_s3::error::ProvideErrorMetadata;
                let conflict = error
                    .as_service_error()
                    .and_then(|value| value.code())
                    .is_some_and(|code| {
                        matches!(code, "PreconditionFailed" | "ConditionalRequestConflict")
                    });
                return Err(if conflict {
                    StorageError::Conflict
                } else {
                    StorageError::Unavailable
                });
            }
        }
        if !self.head_verified(object).await? {
            return Err(StorageError::Corrupt);
        }
        Ok(())
    }

    async fn head_verified(&self, object: &ObjectRef) -> Result<bool, StorageError> {
        if !Self::valid(object) {
            return Err(StorageError::Invalid);
        }
        let response = self
            .client
            .head_object()
            .bucket(&self.bucket)
            .key(Self::key(object))
            .send()
            .await
            .map_err(|_| StorageError::Unavailable)?;
        Ok(Self::head_is_exact(
            object,
            response.content_length(),
            response.content_type(),
            response.content_encoding(),
            response
                .metadata()
                .and_then(|values| values.get("sha256"))
                .map(String::as_str),
        ))
    }

    async fn get_verified(&self, object: &ObjectRef) -> Result<Vec<u8>, StorageError> {
        if !Self::valid(object) {
            return Err(StorageError::Invalid);
        }
        let response = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(Self::key(object))
            .send()
            .await
            .map_err(|_| StorageError::Unavailable)?;
        if !Self::head_is_exact(
            object,
            response.content_length(),
            response.content_type(),
            response.content_encoding(),
            response
                .metadata()
                .and_then(|values| values.get("sha256"))
                .map(String::as_str),
        ) {
            return Err(StorageError::Corrupt);
        }
        // HEAD and Content-Length can both lie: bound the response stream too.
        let mut reader = response.body.into_async_read().take(object.size_bytes + 1);
        let mut bytes = Vec::with_capacity((object.size_bytes as usize).min(65_536));
        reader
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| StorageError::Unavailable)?;
        if bytes.len() as u64 != object.size_bytes
            || sha256_bytes(&bytes) != object.sha256_ciphertext
        {
            return Err(StorageError::Corrupt);
        }
        Ok(bytes)
    }

    async fn presign_verified(
        &self,
        object: &ObjectRef,
        ttl: Duration,
    ) -> Result<SecretValue, StorageError> {
        if !Self::valid(object) || validate_signed_get_ttl(ttl).is_err() {
            return Err(StorageError::Invalid);
        }
        if !self.head_verified(object).await? {
            return Err(StorageError::Corrupt);
        }
        let config =
            PresigningConfig::expires_in(ttl).map_err(|_| StorageError::Invalid)?;
        let signed = self
            .client
            .get_object()
            .bucket(&self.bucket)
            .key(Self::key(object))
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
