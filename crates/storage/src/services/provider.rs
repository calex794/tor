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
