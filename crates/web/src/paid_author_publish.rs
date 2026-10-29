//! Author paid chapter Save: B reserves a hidden immutable version, G seals,
//! registers and actually verifies private S3, B atomically attaches/publishes.
//! No writer endpoint is enabled without an explicit, configured S3-paid router.
use crate::author_chapter_http::PaidChapterPublisher;
use crate::author_chapter_images::stable_image_id;
use crate::paid_author_assets::PgPaidAuthorAssets;
use async_trait::async_trait;
use std::sync::Arc;
use tor_books::{
    repositories::{
        paid_storage::{PaidPublication, PgPaidStorage},
        PgCatalog,
    },
    services::authoring::{AuthorError, SaveChapter},
};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};
use tor_storage::{
    domain::provider::valid_private_bucket,
    repositories::registry::PgStorageRegistry,
    services::{
        crypto::{fresh_private_object_id, seal_chapter_with_keyring, ContentKeyring},
        provider::{upload_and_verify, PrivateObjectStore},
    },
};

pub struct PgPaidAuthorPublisher<S> {
    paid: PgPaidStorage,
    catalog: PgCatalog,
    registry: PgStorageRegistry,
    store: S,
    keyring: Arc<ContentKeyring>,
    bucket: String,
}

impl<S: PrivateObjectStore> PgPaidAuthorPublisher<S> {
    pub fn new(
        catalog: PgCatalog,
        store: S,
        keyring: Arc<ContentKeyring>,
        bucket: String,
    ) -> Result<Self, &'static str> {
        if !valid_private_bucket(&bucket) {
            return Err("Invalid private author S3 bucket");
        }
        let pool = catalog.pool();
        Ok(Self {
            paid: PgPaidStorage::new(pool.clone()),
            catalog,
            registry: PgStorageRegistry::new(pool),
            store,
            keyring,
            bucket,
        })
    }
}

impl<S: PrivateObjectStore> PgPaidAuthorPublisher<S> {
    /// Reusable orchestration for the browser and isolated PG/S3 tests.
    pub async fn publish(&self, actor: i64, request: &SaveChapter) -> Result<(), AuthorError> {
        let pending = self.paid.prepare_new_paid_chapter(actor, request).await?;
        // A lost HTTP response is not another publication. Earlier G objects
        // and purchase-scoped old editions are deliberately left untouched.
        if pending.published {
            return Ok(());
        }
        let version = ChapterVersionRef {
            chapter_id: PositiveId::new(pending.chapter_id)
                .map_err(|_| AuthorError::Unavailable)?,
            version_id: PositiveId::new(pending.version_id)
                .map_err(|_| AuthorError::Unavailable)?,
        };
        let object_id = fresh_private_object_id().map_err(|_| AuthorError::Unavailable)?;
        let sealed =
            seal_chapter_with_keyring(version, object_id.clone(), &request.body, &self.keyring)
                .map_err(|_| AuthorError::Unavailable)?;
        let object = self
            .registry
            .reserve_pending_with_keyring(&sealed, &self.keyring, &self.bucket)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        // Neither a successful PUT nor HEAD alone is sufficient. A failed
        // GET/hash leaves a private pending object for the bounded G GC.
        let receipt = upload_and_verify(&self.store, &object, &sealed.ciphertext)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        self.registry
            .mark_ready(&receipt)
            .await
            .map_err(|_| AuthorError::Unavailable)?;

        let published = self
            .paid
            .attach_verified_and_publish(
                actor,
                request.book_id,
                version,
                &object_id,
                PaidPublication::Publish {
                    expected_current: None,
                },
            )
            .await;
        if published == Err(AuthorError::Conflict) {
            // Two identical tabs can race with different random G objects.
            // Report success only when the exact idempotent B version won.
            let current = self.paid.prepare_new_paid_chapter(actor, request).await?;
            if current.chapter_id == pending.chapter_id
                && current.version_id == pending.version_id
                && current.published
            {
                return Ok(());
            }
        }
        published
    }
}

impl<S: PrivateObjectStore + Clone> PgPaidAuthorPublisher<S> {
    /// One-image author form: final immutable Markdown contains exactly this
    /// server-derived ID before B reserves the version. Only encrypted G
    /// bytes and the version-scoped B asset row are persisted.
    pub async fn publish_illustrated(
        &self,
        actor: i64,
        request: &SaveChapter,
        object_id: &tor_contracts::paid_delivery::ObjectId,
        bytes: &[u8],
        mime: &str,
    ) -> Result<(), AuthorError> {
        if actor <= 0 || stable_image_id(actor, request.book_id, &request.command_key) != *object_id {
            return Err(AuthorError::Invalid);
        }
        let pending = self.paid.prepare_new_paid_chapter(actor, request).await?;
        let pinned = ChapterVersionRef {
            chapter_id: PositiveId::new(pending.chapter_id)
                .map_err(|_| AuthorError::Unavailable)?,
            version_id: PositiveId::new(pending.version_id)
                .map_err(|_| AuthorError::Unavailable)?,
        };
        if pending.published {
            // Lost HTTP response is a true retry ONLY for identical bytes.
            let digest = tor_storage::services::crypto::sha256_bytes(bytes);
            return match tor_books::repositories::paid_assets::PgPaidAssets::new(self.catalog.pool())
                .existing_draft_upload(actor, pinned, 1, mime, &digest)
                .await?
            {
                Some(existing) if existing == *object_id => Ok(()),
                _ => Err(AuthorError::Conflict),
            };
        }
        let uploader = PgPaidAuthorAssets::new(
            self.catalog.clone(),
            self.store.clone(),
            self.keyring.clone(),
            self.bucket.clone(),
        )
        .map_err(|_| AuthorError::Unavailable)?;
        uploader
            .upload_to_draft_with_id(actor, request, 1, bytes, mime, object_id)
            .await?;
        self.publish(actor, request).await
    }
}

#[async_trait]
impl<S: PrivateObjectStore + Clone> PaidChapterPublisher for PgPaidAuthorPublisher<S> {
    async fn publish_new(&self, actor: i64, request: &SaveChapter) -> Result<(), AuthorError> {
        self.publish(actor, request).await
    }

    async fn publish_with_image(
        &self,
        actor: i64,
        request: &SaveChapter,
        object_id: &tor_contracts::paid_delivery::ObjectId,
        bytes: &[u8],
        mime: &str,
    ) -> Result<(), AuthorError> {
        self.publish_illustrated(actor, request, object_id, bytes, mime).await
    }
}
