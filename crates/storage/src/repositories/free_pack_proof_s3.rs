//! DROP-R3b: private full-pack plus sidecar readback for trusted B/G workers.
//!
//! Unlike the frame reader, this bounded full GET is ONLY for offline ingest
//! verification before a PostgreSQL receipt may be created. It never creates
//! any signed URL and must never be wired to the reader HTTP response.

use aws_sdk_s3::Client;
use tokio::io::AsyncReadExt;

use crate::{
    domain::provider::StorageError,
    services::{
        free_chapter::FreeChapterObject,
        free_pack_proof::{verify_full_pack, PackProofTarget, VerifiedPack},
    },
};

pub struct S3PackProofStore {
    client: Client,
}

impl S3PackProofStore {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    pub async fn verify_independent_full_get(
        &self,
        target: &PackProofTarget,
        authoritative_editions: &[FreeChapterObject],
    ) -> Result<VerifiedPack, StorageError> {
        // Validate the *trusted* locator before doing any S3 network I/O.
        // A malformed key or excessive size cannot trigger a full-object GET.
        target.validate()?;
        let pack = self
            .get_exact(&target.bucket, &target.pack_key, target.pack_bytes)
            .await?;
        let manifest = self
            .get_exact(&target.bucket, &target.manifest_key, target.manifest_bytes)
            .await?;
        verify_full_pack(target, authoritative_editions, &pack, &manifest)
    }

    async fn get_exact(
        &self,
        bucket: &str,
        key: &str,
        expected_bytes: u64,
    ) -> Result<Vec<u8>, StorageError> {
        let response = self
            .client
            .get_object()
            .bucket(bucket)
            .key(key)
            .send()
            .await
            .map_err(|_| StorageError::Unavailable)?;
        if response.content_length() != Some(expected_bytes as i64)
            || response.content_encoding().is_some()
        {
            return Err(StorageError::Corrupt);
        }
        let mut reader = response.body.into_async_read().take(expected_bytes + 1);
        let mut bytes = Vec::with_capacity((expected_bytes as usize).min(65_536));
        reader
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| StorageError::Unavailable)?;
        if bytes.len() as u64 != expected_bytes {
            return Err(StorageError::Corrupt);
        }
        Ok(bytes)
    }
}
