//! G/S349: immutable private S3 bytes for FREE chapter editions.
//! A storage receipt is not a reader entitlement or a publication.
//! The B publisher must attach only after this service has verified real GET bytes.
use async_trait::async_trait;
use std::time::Duration;
use tor_contracts::paid_delivery::SecretValue;

use crate::{domain::provider::StorageError, services::crypto::sha256_bytes};

pub const MAX_FREE_MARKDOWN_BYTES: u64 = 200_000;
pub const FREE_MARKDOWN_CONTENT_TYPE: &str = "text/plain; charset=utf-8";

/// Exact B edition identity. The S3 key is server-generated, never browser input.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FreeChapterObject {
    chapter_id: i64,
    version_id: i64,
    sha256: [u8; 32],
    size_bytes: u64,
}

impl FreeChapterObject {
    /// Use the already normalized author Markdown, before saving its B edition.
    pub fn for_markdown(
        chapter_id: i64,
        version_id: i64,
        markdown: &str,
    ) -> Result<Self, StorageError> {
        if !(1..=50_000).contains(&markdown.chars().count()) || markdown.contains('\0') {
            return Err(StorageError::Invalid);
        }
        Self::from_metadata(
            chapter_id,
            version_id,
            sha256_bytes(markdown.as_bytes()),
            markdown.len() as u64,
        )
    }

    /// Rebuild from B's immutable version metadata after authorization.
    /// This is NOT evidence that an S3 object exists or that a reader may access it.
    pub fn from_metadata(
        chapter_id: i64,
        version_id: i64,
        sha256: [u8; 32],
        size_bytes: u64,
    ) -> Result<Self, StorageError> {
        if chapter_id <= 0
            || version_id <= 0
            || !(1..=MAX_FREE_MARKDOWN_BYTES).contains(&size_bytes)
            || sha256 == [0; 32]
        {
            return Err(StorageError::Invalid);
        }
        Ok(Self {
            chapter_id,
            version_id,
            sha256,
            size_bytes,
        })
    }

    pub fn chapter_id(&self) -> i64 {
        self.chapter_id
    }

    pub fn version_id(&self) -> i64 {
        self.version_id
    }

    pub fn sha256(&self) -> &[u8; 32] {
        &self.sha256
    }

    pub fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    pub fn key(&self) -> String {
        format!(
            "private/free-chapters/{}/{}.md",
            self.chapter_id, self.version_id
        )
    }
}

#[async_trait]
pub trait FreeChapterStore: Send + Sync {
    /// Provider must enforce If-None-Match: *, not check-then-put.
    async fn put_immutable(
        &self,
        object: &FreeChapterObject,
        bytes: &[u8],
    ) -> Result<(), StorageError>;

    /// Checks actual bounded GET bytes, not just a possibly forged HEAD.
    async fn get_verified(&self, object: &FreeChapterObject) -> Result<Vec<u8>, StorageError>;

    /// Only trusted, reauthorized B/C readers may request a short URL.
    /// The browser must check the expected SHA256 before rendering.
    async fn presign_verified(
        &self,
        object: &FreeChapterObject,
        ttl: Duration,
    ) -> Result<SecretValue, StorageError>;
}

#[derive(Clone, Debug)]
pub struct VerifiedFreeChapter {
    object: FreeChapterObject,
}

impl VerifiedFreeChapter {
    pub fn object(&self) -> &FreeChapterObject {
        &self.object
    }
}

/// A retry after PUT conflict succeeds only if GET confirms the exact bytes.
/// No PostgreSQL publication or rights change happens in this G service.
pub async fn upload_and_verify(
    store: &impl FreeChapterStore,
    object: &FreeChapterObject,
    markdown: &str,
) -> Result<VerifiedFreeChapter, StorageError> {
    let bytes = markdown.as_bytes();
    if bytes.len() as u64 != object.size_bytes
        || sha256_bytes(bytes) != object.sha256
        || !(1..=50_000).contains(&markdown.chars().count())
        || markdown.contains('\0')
    {
        return Err(StorageError::Invalid);
    }
    match store.put_immutable(object, bytes).await {
        Ok(()) | Err(StorageError::Conflict) => {}
        Err(error) => return Err(error),
    }
    if store.get_verified(object).await? != bytes {
        return Err(StorageError::Corrupt);
    }
    Ok(VerifiedFreeChapter {
        object: object.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct MemoryStore {
        stored: Mutex<Option<Vec<u8>>>,
    }

    #[async_trait]
    impl FreeChapterStore for MemoryStore {
        async fn put_immutable(
            &self,
            _: &FreeChapterObject,
            bytes: &[u8],
        ) -> Result<(), StorageError> {
            let mut stored = self.stored.lock().unwrap();
            if stored.is_some() {
                return Err(StorageError::Conflict);
            }
            *stored = Some(bytes.to_vec());
            Ok(())
        }

        async fn get_verified(&self, _: &FreeChapterObject) -> Result<Vec<u8>, StorageError> {
            self.stored
                .lock()
                .unwrap()
                .clone()
                .ok_or(StorageError::Unavailable)
        }

        async fn presign_verified(
            &self,
            _: &FreeChapterObject,
            _: Duration,
        ) -> Result<SecretValue, StorageError> {
            Err(StorageError::Unavailable)
        }
    }

    #[tokio::test]
    async fn free_version_bytes_are_bounded_immutable_and_replay_safe() {
        let text = "Глава с эмодзи 😀 и 中文";
        let object = FreeChapterObject::for_markdown(10, 20, text).unwrap();
        assert_eq!(object.key(), "private/free-chapters/10/20.md");
        assert_eq!(object.size_bytes(), text.len() as u64);
        let store = MemoryStore::default();
        assert_eq!(
            upload_and_verify(&store, &object, text)
                .await
                .unwrap()
                .object(),
            &object
        );
        assert!(upload_and_verify(&store, &object, text).await.is_ok());
        assert_eq!(
            upload_and_verify(&store, &object, "другой текст")
                .await
                .err(),
            Some(StorageError::Invalid)
        );
        *store.stored.lock().unwrap() = Some(b"corrupt".to_vec());
        assert_eq!(
            upload_and_verify(&store, &object, text).await.err(),
            Some(StorageError::Corrupt)
        );
        assert!(FreeChapterObject::for_markdown(0, 20, text).is_err());
        assert!(FreeChapterObject::for_markdown(10, -2, text).is_err());
        assert!(FreeChapterObject::for_markdown(10, 20, "").is_err());
        assert!(FreeChapterObject::for_markdown(10, 20, "x\0y").is_err());
        assert!(FreeChapterObject::for_markdown(10, 20, &"x".repeat(50_001)).is_err());
        assert!(FreeChapterObject::for_markdown(10, 20, &"😀".repeat(50_000)).is_ok());
        assert!(FreeChapterObject::from_metadata(10, 20, [0; 32], 10).is_err());
    }
}
