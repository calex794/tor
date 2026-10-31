//! Optional S3-compatible adapter used by G's worker after trusted upload/finalize.
//! No public buckets, anonymous credentials or plaintext proxy through tor-web.
#![cfg(feature = "s3")]
use crate::domain::cover::CoverError;
use crate::services::cover_jobs::CoverObjectStore;
use async_trait::async_trait;
use aws_sdk_s3::{presigning::PresigningConfig, primitives::ByteStream, Client};
use std::time::Duration;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

#[derive(Clone)]
pub struct S3CoverStore {
    client: Client,
    private_bucket: String,
    direct_origin: Option<String>,
}
impl S3CoverStore {
    pub fn new(client: Client, private_bucket: String) -> Result<Self, CoverError> {
        if private_bucket.trim().is_empty() || private_bucket.chars().any(char::is_whitespace) {
            return Err(CoverError::Invalid);
        }
        Ok(Self {
            client,
            private_bucket,
            direct_origin: None,
        })
    }
    /// The public origin is pinned by the operator, never supplied by a browser.
    /// Presigned host must match; no redirect to a backend-only S3 address.
    pub fn with_direct_origin(mut self, origin: String) -> Result<Self, CoverError> {
        if !origin.starts_with("https://") || origin.ends_with('/') || origin.contains('?') || origin.contains('#') || origin.contains('@') || origin.contains('/') && origin[8..].contains('/') {
            return Err(CoverError::Invalid);
        }
        self.direct_origin = Some(origin);
        Ok(self)
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
    fn direct_read_origin(&self) -> Option<&str> {
        self.direct_origin.as_deref()
    }

    async fn presign_variant_get(&self, key: &str, ttl: Duration) -> Result<String, CoverError> {
        let origin = self.direct_origin.as_deref().ok_or(CoverError::Forbidden)?;
        if !key.starts_with("staging/covers/variants/") || !Self::permitted(key)
            || ttl.as_secs() == 0 || ttl.as_secs() > 120
        {
            return Err(CoverError::Invalid);
        }
        let signed = self.client.get_object()
            .bucket(&self.private_bucket).key(key)
            .presigned(PresigningConfig::expires_in(ttl).map_err(|_| CoverError::Invalid)?)
            .await.map_err(|_| CoverError::Unavailable)?;
        let url = signed.uri().to_string();
        if !url.starts_with(&format!("{origin}/")) {
            return Err(CoverError::Unavailable);
        }
        Ok(url)
    }

    async fn put_source_verified(
        &self,
        key: &str,
        bytes: &[u8],
        mime: &str,
        sha256: &[u8; 32],
    ) -> Result<(), CoverError> {
        let actual: [u8; 32] = Sha256::digest(bytes).into();
        // The HTTP layer also validates and decodes images, but direct
        // callers must not be able to forge a stored source MIME either.
        let mime_matches_bytes = match mime {
            "image/png" => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            "image/jpeg" => bytes.starts_with(&[0xff, 0xd8, 0xff]),
            "image/webp" => {
                bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP"
            }
            _ => false,
        };
        if !key.starts_with("staging/covers/sources/")
            || !Self::permitted(key)
            || !mime_matches_bytes
            || bytes.is_empty()
            || bytes.len() > 8 * 1024 * 1024
            || actual != *sha256
        {
            return Err(CoverError::Invalid);
        }
        let digest = Self::hash_hex(bytes);
        self.client
            .put_object()
            .bucket(&self.private_bucket)
            .key(key)
            .content_type(mime)
            .metadata("sha256", digest.as_str())
            .body(ByteStream::from(bytes.to_vec()))
            .send()
            .await
            .map_err(|_| CoverError::Unavailable)?;
        let head = self
            .client
            .head_object()
            .bucket(&self.private_bucket)
            .key(key)
            .send()
            .await
            .map_err(|_| CoverError::Unavailable)?;
        if head.content_length() != Some(bytes.len() as i64)
            || head.content_type() != Some(mime)
            || head
                .metadata()
                .and_then(|metadata| metadata.get("sha256"))
                .is_none_or(|stored| stored != &digest)
        {
            return Err(CoverError::Unavailable);
        }
        let stored = self.get_bounded(key, bytes.len()).await?;
        let actual: [u8; 32] = Sha256::digest(&stored).into();
        if stored.len() != bytes.len() || actual != *sha256 {
            return Err(CoverError::Unavailable);
        }
        Ok(())
    }

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
