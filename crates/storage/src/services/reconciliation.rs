//! G-09/S3-RECON-01: read-only, bounded inventory of approved private namespaces.
//! No deletion, object GET, public URL, key material or signed URL is exposed.
use async_trait::async_trait;
use std::time::SystemTime;

use crate::domain::provider::StorageError;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivateScope {
    Chapter,
    ChapterAsset,
}

impl PrivateScope {
    pub fn prefix(self) -> &'static str {
        match self {
            Self::Chapter => "private/chapter-versions/",
            Self::ChapterAsset => "private/chapter-assets/",
        }
    }

    pub fn kind(self) -> &'static str {
        match self {
            Self::Chapter => "chapter",
            Self::ChapterAsset => "chapter_asset",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ListedPrivateObject {
    pub key: String,
    pub size_bytes: Option<i64>,
    pub modified_at: Option<SystemTime>,
}

#[derive(Clone, Debug)]
pub struct PrivateInventoryPage {
    pub objects: Vec<ListedPrivateObject>,
    pub continuation: Option<String>,
}

/// Only a trusted, configured private store implements the production port.
/// Continuation tokens are opaque and must never enter HTTP or shared logs.
#[async_trait]
pub trait PrivateInventoryLister: Send + Sync {
    fn private_bucket(&self) -> &str;

    async fn list_private_page(
        &self,
        scope: PrivateScope,
        continuation: Option<&str>,
    ) -> Result<PrivateInventoryPage, StorageError>;
}
