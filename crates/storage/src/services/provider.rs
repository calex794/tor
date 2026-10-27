//! G-only private object store port. A client-supplied bucket/key is never input.
use async_trait::async_trait;
use std::time::Duration;
use tor_contracts::paid_delivery::{ObjectRef, SecretValue};

use crate::domain::provider::StorageError;

#[async_trait]
pub trait PrivateObjectStore: Send + Sync {
    /// S3 put-if-absent. Repeated publication may not replace immutable bytes.
    async fn put_immutable(
        &self,
        object: &ObjectRef,
        ciphertext: &[u8],
    ) -> Result<(), StorageError>;

    /// HEAD metadata checks are a preflight, not evidence of actual S3 bytes.
    async fn head_verified(&self, object: &ObjectRef) -> Result<bool, StorageError>;

    /// Download and check actual ciphertext size and SHA-256 before PG attach.
    async fn get_verified(&self, object: &ObjectRef) -> Result<Vec<u8>, StorageError>;

    /// G-only signer. Callers MUST recheck D entitlement and exact B edition.
    /// This method is intentionally not wired to an HTTP route yet.
    async fn presign_verified(
        &self,
        object: &ObjectRef,
        ttl: Duration,
    ) -> Result<SecretValue, StorageError>;
}

/// A G-only receipt produced after an actual bounded ciphertext GET and
/// SHA-256 comparison. No arbitrary caller can instantiate this receipt.
#[derive(Clone, Debug)]
pub struct VerifiedObject {
    object: ObjectRef,
}

impl VerifiedObject {
    pub fn object(&self) -> &ObjectRef {
        &self.object
    }
}

/// Reserve pending in PostgreSQL first, then call this after the S3 PUT.
/// On crash after a successful PUT, a retry may receive Conflict; it must
/// re-read and compare bytes, not overwrite the immutable S3 key.
pub async fn upload_and_verify(
    store: &impl PrivateObjectStore,
    object: &ObjectRef,
    ciphertext: &[u8],
) -> Result<VerifiedObject, StorageError> {
    use crate::services::crypto::sha256_bytes;

    if ciphertext.len() as u64 != object.size_bytes
        || sha256_bytes(ciphertext) != object.sha256_ciphertext
    {
        return Err(StorageError::Invalid);
    }
    match store.put_immutable(object, ciphertext).await {
        Ok(()) | Err(StorageError::Conflict) => {}
        Err(error) => return Err(error),
    }
    let actual = store.get_verified(object).await?;
    if actual.len() as u64 != object.size_bytes
        || sha256_bytes(&actual) != object.sha256_ciphertext
    {
        return Err(StorageError::Corrupt);
    }
    Ok(VerifiedObject {
        object: object.clone(),
    })
}
