//! S023 private staging storage for author-confirmed chapter imports.
use async_trait::async_trait;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChapterImportStoreError {
    Invalid,
    Conflict,
    Corrupt,
    Unavailable,
}

/// Private object storage capability. Implementations must verify both provider
/// metadata and streamed bytes; callers never choose a bucket or public URL.
#[async_trait]
pub trait ChapterImportObjectStore: Send + Sync {
    async fn put_verified(
        &self,
        key: &str,
        bytes: &[u8],
        sha256: &[u8; 32],
    ) -> Result<(), ChapterImportStoreError>;

    async fn get_verified(
        &self,
        key: &str,
        expected_size: i64,
        sha256: &[u8; 32],
        max_bytes: usize,
    ) -> Result<Vec<u8>, ChapterImportStoreError>;

    async fn delete(&self, key: &str) -> Result<(), ChapterImportStoreError>;
}
