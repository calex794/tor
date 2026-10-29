//! S065/B/G: owner-only private image preparation for an immutable paid draft.
//! No public route or media URL is exposed here. Publication remains B-owned.
use std::sync::Arc;
use tor_books::{
    repositories::{paid_assets::PgPaidAssets, paid_storage::PgPaidStorage, PgCatalog},
    services::authoring::{AuthorError, SaveChapter},
};
use tor_contracts::paid_delivery::{ChapterVersionRef, ObjectId, PositiveId};
use tor_storage::{
    domain::provider::valid_private_bucket,
    repositories::registry::PgStorageRegistry,
    services::{
        crypto::{fresh_private_object_id, seal_asset_with_keyring, ContentKeyring, CryptoError},
        provider::{upload_and_verify, PrivateObjectStore},
    },
};

pub struct PgPaidAuthorAssets<S> {
    paid: PgPaidStorage,
    assets: PgPaidAssets,
    registry: PgStorageRegistry,
    store: S,
    keyring: Arc<ContentKeyring>,
    bucket: String,
}

impl<S: PrivateObjectStore> PgPaidAuthorAssets<S> {
    pub fn new(
        catalog: PgCatalog,
        store: S,
        keyring: Arc<ContentKeyring>,
        bucket: String,
    ) -> Result<Self, &'static str> {
        if !valid_private_bucket(&bucket) {
            return Err("Invalid private author media bucket");
        }
        let pool = catalog.pool();
        Ok(Self {
            paid: PgPaidStorage::new(pool.clone()),
            assets: PgPaidAssets::new(pool.clone()),
            registry: PgStorageRegistry::new(pool),
            store,
            keyring,
            bucket,
        })
    }

    /// First reserve B's unpublished paid version from the SAME immutable
    /// SaveChapter command that will be published. An unknown/foreign author
    /// is rejected before any private S3 upload. This library path is NOT a
    /// browser endpoint; a future author form must provide a stable mapping
    /// between its placeholders and the returned server-minted object ID.
    pub async fn upload_to_draft(
        &self,
        actor: i64,
        request: &SaveChapter,
        ordinal: i16,
        bytes: &[u8],
        mime: &str,
    ) -> Result<ObjectId, AuthorError> {
        if !(1..=64).contains(&ordinal) {
            return Err(AuthorError::Invalid);
        }
        let media_type = match mime {
            "image/png" => "image/png",
            "image/jpeg" => "image/jpeg",
            "image/webp" => "image/webp",
            _ => return Err(AuthorError::Invalid),
        };
        // G rejects invalid magic, zero-sized objects, excessive byte counts
        // and unsupported types before S3. H separately validates dimensions.
        if bytes.is_empty() || bytes.len() > 8 * 1024 * 1024 {
            return Err(AuthorError::Invalid);
        }
        let pending = self.paid.prepare_new_paid_chapter(actor, request).await?;
        if pending.published {
            return Err(AuthorError::Conflict);
        }
        let pinned = ChapterVersionRef {
            chapter_id: PositiveId::new(pending.chapter_id)
                .map_err(|_| AuthorError::Unavailable)?,
            version_id: PositiveId::new(pending.version_id)
                .map_err(|_| AuthorError::Unavailable)?,
        };
        // The server, not the browser, mints the private object ID. Its
        // canonical chapter/version/hash AAD is immutable after G sealing.
        let digest = tor_storage::services::crypto::sha256_bytes(bytes);
        if let Some(existing) = self
            .assets
            .existing_draft_upload(actor, pinned, ordinal, media_type, &digest)
            .await?
        {
            return Ok(existing);
        }
        let object_id = fresh_private_object_id().map_err(|_| AuthorError::Unavailable)?;
        let sealed =
            seal_asset_with_keyring(pinned, object_id.clone(), bytes, media_type, &self.keyring)
                .map_err(|error| match error {
                    CryptoError::InvalidMaterial => AuthorError::Invalid,
                    _ => AuthorError::Unavailable,
                })?;
        let object = self
            .registry
            .reserve_pending_asset(&sealed, &self.keyring, &self.bucket)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        // PUT alone is not a trusted image: verify actual S3 GET and digest
        // before G-ready, then let B atomically bind it to the exact draft.
        let receipt = upload_and_verify(&self.store, &object, &sealed.ciphertext)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        self.registry
            .mark_ready(&receipt)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        match self
            .assets
            .attach_prepared(actor, pinned, ordinal, media_type, &object_id)
            .await
        {
            Ok(()) => Ok(object_id),
            Err(AuthorError::Conflict) => self
                .assets
                .existing_draft_upload(actor, pinned, ordinal, media_type, &digest)
                .await?
                .ok_or(AuthorError::Conflict),
            Err(error) => Err(error),
        }
    }
}
