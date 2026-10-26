//! Optional S3-compatible adapter used by G's worker after trusted upload/finalize.
//! No public buckets, anonymous credentials or plaintext proxy through tor-web.
#![cfg(feature = "s3")]
use crate::domain::cover::CoverError;
use crate::services::cover_jobs::CoverObjectStore;
use async_trait::async_trait;
use aws_sdk_s3::{primitives::ByteStream, Client};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

#[derive(Clone)]
pub struct S3CoverStore {
    client: Client,
    private_bucket: String,
}
impl S3CoverStore {
    pub fn new(client: Client, private_bucket: String) -> Result<Self, CoverError> {
        if private_bucket.trim().is_empty() || private_bucket.chars().any(char::is_whitespace) {
            return Err(CoverError::Invalid);
        }
        Ok(Self {
            client,
            private_bucket,
        })
    }
    fn permitted(key: &str) -> bool {
        key.starts_with("staging/covers/")
            && key.len() <= 512
            && !key.contains("..")
            && !key.contains("//")
            && key
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.'))
    }
    fn hash_hex(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[async_trait]
impl CoverObjectStore for S3CoverStore {
    async fn get_bounded(&self, key: &str, max_bytes: usize) -> Result<Vec<u8>, CoverError> {
        if !Self::permitted(key) || max_bytes == 0 {
            return Err(CoverError::Invalid);
        }
        let output = self
            .client
            .get_object()
            .bucket(&self.private_bucket)
            .key(key)
            .send()
            .await
            .map_err(|_| CoverError::Unavailable)?;
        if output
            .content_length()
            .is_some_and(|size| size < 0 || size as u64 > max_bytes as u64)
        {
            return Err(CoverError::Invalid);
        }
        // Content-Length is untrusted: the explicit take() bound also handles
        // unknown-length and dishonest streaming responses safely.
        let mut reader = output.body.into_async_read().take(max_bytes as u64 + 1);
        let mut buffer = Vec::with_capacity(max_bytes.min(64 * 1024));
        reader
            .read_to_end(&mut buffer)
            .await
            .map_err(|_| CoverError::Unavailable)?;
        if buffer.len() > max_bytes {
            return Err(CoverError::Invalid);
        }
        Ok(buffer)
    }

    async fn put_verified(&self, key: &str, bytes: &[u8]) -> Result<(), CoverError> {
        if !key.starts_with("staging/covers/variants/")
            || !Self::permitted(key)
            || bytes.is_empty()
            || bytes.len() > 8 * 1024 * 1024
        {
            return Err(CoverError::Invalid);
        }
        let hash = Self::hash_hex(bytes);
        self.client
            .put_object()
            .bucket(&self.private_bucket)
            .key(key)
            .content_type("image/webp")
            .metadata("sha256", &hash)
            .body(ByteStream::from(bytes.to_owned()))
            .send()
            .await
            .map_err(|_| CoverError::Unavailable)?;
        let sha: [u8; 32] = Sha256::digest(bytes).into();
        if !self.exists_verified(key, &sha, bytes.len() as i64).await? {
            return Err(CoverError::Unavailable);
        }
        Ok(())
    }

    async fn exists_verified(
        &self,
        key: &str,
        sha256: &[u8; 32],
        size: i64,
    ) -> Result<bool, CoverError> {
        if !Self::permitted(key) || size <= 0 {
            return Err(CoverError::Invalid);
        }
        let head = self
            .client
            .head_object()
            .bucket(&self.private_bucket)
            .key(key)
            .send()
            .await
            .map_err(|_| CoverError::Unavailable)?;
        let expected: String = sha256.iter().map(|b| format!("{b:02x}")).collect();
        Ok(head.content_length() == Some(size)
            && head.content_type() == Some("image/webp")
            && head
                .metadata()
                .and_then(|m| m.get("sha256"))
                .is_some_and(|s| s == &expected))
    }
}
