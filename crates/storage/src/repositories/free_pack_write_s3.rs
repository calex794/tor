//! DROP-R4b2: private conditional PACK and sidecar writes.
//! A lost PUT ACK is ambiguous. Only independent R3b full GET may turn the
//! proposed bytes into a trusted placement receipt; no PG or HTTP route here.
use aws_sdk_s3::{primitives::ByteStream, Client};

use super::free_pack_proof_s3::S3PackProofStore;
use crate::{
    domain::provider::StorageError,
    services::{free_pack_proof::VerifiedPack, free_pack_write::PreparedFreePack},
};

#[derive(Clone)]
pub struct S3FreePackWriter {
    client: Client,
}

impl S3FreePackWriter {
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    pub async fn upload_and_verify(
        &self,
        prepared: &PreparedFreePack,
    ) -> Result<VerifiedPack, StorageError> {
        let target = prepared.target();
        target.validate()?;
        // Both immutable writes use If-None-Match:*. A 412 or lost ACK is not
        // proof of success, and must never trigger an overwrite or a PG commit.
        self.put_candidate(
            &target.bucket,
            &target.pack_key,
            prepared.pack_bytes(),
            "application/zstd",
        )
        .await;
        self.put_candidate(
            &target.bucket,
            &target.manifest_key,
            prepared.manifest_bytes(),
            "application/json",
        )
        .await;
        let verified = S3PackProofStore::new(self.client.clone())
            .verify_independent_full_get(target, prepared.editions())
            .await?;
        if verified.placement_receipt() != *prepared.receipt() {
            return Err(StorageError::Corrupt);
        }
        Ok(verified)
    }

    async fn put_candidate(&self, bucket: &str, key: &str, bytes: &[u8], content_type: &str) {
        // All failures are deliberately treated as ambiguous, including
        // 412/409: the subsequent exact full GET decides whether the
        // object was already committed with the expected immutable bytes.
        let _ = self
            .client
            .put_object()
            .bucket(bucket)
            .key(key)
            .content_type(content_type)
            .cache_control("private, no-store")
            .if_none_match("*")
            .body(ByteStream::from(bytes.to_vec()))
            .send()
            .await;
    }
}
