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

/// The result of an authenticated HEAD against a canonical private object key.
/// Missing is returned ONLY for a provider-confirmed 404/NoSuchKey, never 403,
/// timeouts, invalid credentials or a transport error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisteredHeadResult {
    Present,
    Missing,
    MetadataMismatch,
}

/// Private read-only capability for reverse (PostgreSQL -> S3) reconciliation.
/// No caller-provided S3 path, signed URL, object body or deletion method.
#[async_trait]
pub trait PrivateRegisteredHead: Send + Sync {
    fn private_bucket(&self) -> &str;

    async fn head_registered(
        &self,
        object: &tor_contracts::paid_delivery::ObjectRef,
    ) -> Result<RegisteredHeadResult, StorageError>;
}

/// A bounded GET checks ciphertext bytes, unlike HEAD metadata alone.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegisteredBodyResult {
    Verified,
    Missing,
    BodyMismatch,
    MetadataMismatch,
}

/// Fixed per-object cap, independent of caller-controlled inputs.
pub const G09_BODY_GET_MAX_BYTES: u64 = 4 * 1024 * 1024;

/// No returned ciphertext, raw key, signed URL or user-supplied bucket.
#[async_trait]
pub trait PrivateRegisteredBodyProbe: Send + Sync {
    fn private_bucket(&self) -> &str;

    async fn probe_registered_body(
        &self,
        object: &tor_contracts::paid_delivery::ObjectRef,
    ) -> Result<RegisteredBodyResult, StorageError>;
}
