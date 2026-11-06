//! Optional S3-compatible adapter used by G's worker after trusted upload/finalize.
//! No public buckets, anonymous credentials or plaintext proxy through tor-web.
#![cfg(feature = "s3")]
use crate::domain::cover::CoverError;
use crate::services::cover_jobs::CoverObjectStore;
use async_trait::async_trait;
use aws_sdk_s3::{presigning::PresigningConfig, primitives::ByteStream, Client};
use sha2::{Digest, Sha256};
use std::time::Duration;
use tokio::io::AsyncReadExt;

#[derive(Clone)]
pub struct S3CoverStore {
    client: Client,
    private_bucket: String,
    direct_origin: Option<String>,
    public_cdn: Option<(String, String)>,
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
            public_cdn: None,
        })
    }
    /// The operator configures the exact browser-visible S3 origin.
    /// No client-supplied URL, credentials or internal S3 endpoint escapes.
    pub fn with_direct_origin(mut self, origin: String) -> Result<Self, CoverError> {
        let local_test =
            origin.starts_with("http://127.0.0.1:") || origin.starts_with("http://localhost:");
        let secure = origin.starts_with("https://");
        let host_part = if secure {
            &origin[8..]
        } else if local_test {
            &origin[7..]
        } else {
            ""
        };
        if host_part.is_empty()
            || host_part.contains('/')
            || host_part.contains('@')
            || host_part.contains('?')
            || host_part.contains('#')
            || host_part.bytes().any(|b| b.is_ascii_whitespace())
            || origin.ends_with('/')
        {
            return Err(CoverError::Invalid);
        }
        self.direct_origin = Some(origin);
        Ok(self)
    }

    /// The Yandex CDN source bucket contains ONLY approved, immutable public
    /// variants. It must never equal the private source/paid-content bucket.
    pub fn with_public_cdn(mut self, bucket: String, origin: String) -> Result<Self, CoverError> {
        let hostname = origin.strip_prefix("https://").ok_or(CoverError::Invalid)?;
        if bucket.is_empty()
            || bucket == self.private_bucket
            || !bucket
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'.')
            || hostname.is_empty()
            || hostname.starts_with('.')
            || hostname.ends_with('.')
            || hostname.contains('/')
            || hostname.contains(':')
            || hostname.contains('@')
            || hostname.contains('?')
            || hostname.contains('#')
            || hostname.bytes().any(|c| c.is_ascii_whitespace())
        {
            return Err(CoverError::Invalid);
        }
        self.public_cdn = Some((bucket, origin));
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
    /// The caller supplies only a book/version/digest identity, never a path.
    /// Derive precisely the same immutable key for publication and revocation.
    fn immutable_public_key(
        book_id: u64,
        revision: i64,
        variant: &str,
        sha256: &[u8; 32],
    ) -> Result<String, CoverError> {
        if book_id == 0 || revision <= 0 || !matches!(variant, "main" | "card") {
            return Err(CoverError::Invalid);
        }
        let hash = sha256
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        Ok(format!(
            "public/covers/{book_id}/{revision}/{variant}-{hash}.webp"
        ))
    }

    fn hash_hex(bytes: &[u8]) -> String {
        let digest = Sha256::digest(bytes);
        digest.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[async_trait]
impl CoverObjectStore for S3CoverStore {
    fn public_cdn_origin(&self) -> Option<&str> {
        self.public_cdn.as_ref().map(|(_, origin)| origin.as_str())
    }

    async fn publish_public_cover(
        &self,
        book_id: u64,
        revision: i64,
        variant: &str,
        source_key: &str,
        sha256: &[u8; 32],
        size_bytes: i64,
    ) -> Result<String, CoverError> {
        let (bucket, origin) = self.public_cdn.as_ref().ok_or(CoverError::Forbidden)?;
        let public_key = Self::immutable_public_key(book_id, revision, variant, sha256)?;
        if !source_key.starts_with("staging/covers/variants/")
            || !Self::permitted(source_key)
            || !(1..=8 * 1024 * 1024).contains(&size_bytes)
        {
            return Err(CoverError::Invalid);
        }
        let verified = self.exists_verified(source_key, sha256, size_bytes).await?;
        if !verified {
            return Err(CoverError::Unavailable);
        }
        let hash = sha256
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        self.client
            .copy_object()
            .bucket(bucket)
            .key(&public_key)
            .copy_source(format!("{}/{source_key}", self.private_bucket))
            .metadata_directive(aws_sdk_s3::types::MetadataDirective::Replace)
            .content_type("image/webp")
            .cache_control("public, max-age=31536000, immutable")
            .metadata("sha256", &hash)
            .send()
            .await
            .map_err(|_| CoverError::Unavailable)?;
        let head = self
            .client
            .head_object()
            .bucket(bucket)
            .key(&public_key)
            .send()
            .await
            .map_err(|_| CoverError::Unavailable)?;
        if head.content_length() != Some(size_bytes)
            || head.content_type() != Some("image/webp")
            || head.metadata().and_then(|meta| meta.get("sha256")) != Some(&hash)
        {
            return Err(CoverError::Unavailable);
        }
        Ok(format!("{origin}/{public_key}"))
    }

    async fn remove_public_cover(
        &self,
        book_id: u64,
        revision: i64,
        variant: &str,
        sha256: &[u8; 32],
    ) -> Result<String, CoverError> {
        let (bucket, origin) = self.public_cdn.as_ref().ok_or(CoverError::Forbidden)?;
        let public_key = Self::immutable_public_key(book_id, revision, variant, sha256)?;
        // Exact-key delete only. It cannot touch the private bucket or accept
        // an arbitrary path from a caller. S3 deletion is idempotent.
        // CDN invalidation is a separate, durable lifecycle operation.
        self.client
            .delete_object()
            .bucket(bucket)
            .key(&public_key)
            .send()
            .await
            .map_err(|_| CoverError::Unavailable)?;
        Ok(format!("{origin}/{public_key}"))
    }

    async fn remove_private_staging_variant(&self, key: &str) -> Result<(), CoverError> {
        const PREFIX: &str = "staging/covers/variants/";
        if !key.starts_with(PREFIX) || key.len() <= PREFIX.len() || !Self::permitted(key) {
            return Err(CoverError::Invalid);
        }
        // The B-owned ledger selected this exact immutable key from PostgreSQL.
        // This method never removes a source, a paid object or a CDN origin copy.
        self.client
            .delete_object()
            .bucket(&self.private_bucket)
            .key(key)
            .send()
            .await
            .map_err(|_| CoverError::Unavailable)?;
        Ok(())
    }

    fn direct_read_origin(&self) -> Option<&str> {
        self.direct_origin.as_deref()
    }

    async fn presign_variant_get(&self, key: &str, ttl: Duration) -> Result<String, CoverError> {
        let origin = self.direct_origin.as_deref().ok_or(CoverError::Forbidden)?;
        if !key.starts_with("staging/covers/variants/")
            || !Self::permitted(key)
            || ttl.as_secs() == 0
            || ttl.as_secs() > 120
        {
            return Err(CoverError::Invalid);
        }
        let signed = self
            .client
            .get_object()
            .bucket(&self.private_bucket)
            .key(key)
            .presigned(PresigningConfig::expires_in(ttl).map_err(|_| CoverError::Invalid)?)
            .await
            .map_err(|_| CoverError::Unavailable)?;
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

#[cfg(test)]
mod direct_cover_tests {
    use super::*;
    use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};

    fn store() -> S3CoverStore {
        let config = aws_sdk_s3::Config::builder()
            .behavior_version(BehaviorVersion::latest())
            .region(Region::new("test-region"))
            .credentials_provider(Credentials::new("test", "test", None, None, "unit"))
            .endpoint_url("http://127.0.0.1:9000")
            .force_path_style(true)
            .build();
        S3CoverStore::new(Client::from_conf(config), "private-covers".into()).unwrap()
    }

    #[tokio::test]
    async fn yandex_public_cdn_is_disabled_and_requires_an_isolated_bucket() {
        let disabled = store();
        assert_eq!(disabled.public_cdn_origin(), None);
        assert!(matches!(
            disabled
                .remove_public_cover(42, 1, "card", &[0_u8; 32])
                .await,
            Err(CoverError::Forbidden)
        ));
        assert!(matches!(
            disabled
                .publish_public_cover(
                    42,
                    1,
                    "card",
                    "staging/covers/variants/test.webp",
                    &[0_u8; 32],
                    100,
                )
                .await,
            Err(CoverError::Forbidden)
        ));
        for (bucket, origin) in [
            ("private-covers", "https://cdn.example"),
            ("public-covers", "http://cdn.example"),
            ("public-covers", "https://cdn.example/path"),
            ("public-covers", "https://name@cdn.example"),
            ("public-covers", "https://cdn.example?x=1"),
            ("public covers", "https://cdn.example"),
        ] {
            assert!(
                store()
                    .with_public_cdn(bucket.into(), origin.into())
                    .is_err(),
                "invalid Yandex CDN configuration must fail closed"
            );
        }
        let enabled = store()
            .with_public_cdn("public-covers".into(), "https://cdn.example".into())
            .unwrap();
        assert_eq!(enabled.public_cdn_origin(), Some("https://cdn.example"));
        for (book_id, revision, variant) in [
            (0, 1, "card"),
            (1, 0, "main"),
            (1, -1, "main"),
            (1, 1, "raw"),
            (1, 1, "card/../source"),
        ] {
            assert!(matches!(
                enabled
                    .remove_public_cover(book_id, revision, variant, &[0_u8; 32])
                    .await,
                Err(CoverError::Invalid)
            ));
        }
        for (book_id, revision, variant, key, size) in [
            (0, 1, "card", "staging/covers/variants/a.webp", 10),
            (1, 0, "card", "staging/covers/variants/a.webp", 10),
            (1, 1, "raw", "staging/covers/variants/a.webp", 10),
            (1, 1, "card", "staging/covers/sources/secret.png", 10),
            (1, 1, "main", "staging/covers/variants/../secret.webp", 10),
            (1, 1, "card", "staging/covers/variants/a.webp", 0),
        ] {
            assert!(matches!(
                enabled
                    .publish_public_cover(book_id, revision, variant, key, &[0_u8; 32], size)
                    .await,
                Err(CoverError::Invalid)
            ));
        }
    }

    #[tokio::test]
    async fn direct_covers_default_off_and_reject_untrusted_keys() {
        let disabled = store();
        assert_eq!(disabled.direct_read_origin(), None);
        assert!(matches!(
            disabled
                .presign_variant_get("staging/covers/variants/safe.webp", Duration::from_secs(60))
                .await,
            Err(CoverError::Forbidden)
        ));

        for origin in [
            "http://example.com",
            "https://example.com/path",
            "https://name@example.com",
            "https://example.com?x=1",
            "https://example.com/",
            "ftp://example.com",
        ] {
            assert!(store().with_direct_origin(origin.into()).is_err());
        }

        let enabled = store()
            .with_direct_origin("http://127.0.0.1:9000".into())
            .unwrap();
        assert_eq!(enabled.direct_read_origin(), Some("http://127.0.0.1:9000"));
        for key in [
            "staging/covers/sources/private.png",
            "staging/covers/variants/../private.webp",
            "staging/covers/variants//bad.webp",
        ] {
            assert!(matches!(
                enabled
                    .presign_variant_get(key, Duration::from_secs(60))
                    .await,
                Err(CoverError::Invalid)
            ));
        }
        for ttl in [Duration::ZERO, Duration::from_secs(121)] {
            assert!(matches!(
                enabled
                    .presign_variant_get("staging/covers/variants/valid.webp", ttl)
                    .await,
                Err(CoverError::Invalid)
            ));
        }
    }
}
