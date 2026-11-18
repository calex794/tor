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
    services::authoring::{AuthorError, ChapterAccess, OwnedChapter, SaveChapter},
};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};
use tor_storage::{
    domain::provider::valid_private_bucket,
    repositories::registry::PgStorageRegistry,
    services::{
        crypto::{
            fresh_private_object_id, seal_chapter_with_keyring, ContentKeyring, EncryptedChapter,
        },
        provider::{upload_and_verify, PrivateObjectStore},
    },
};

#[path = "paid_author_publish/illustrations.rs"]
mod illustrations;

pub struct PgPaidAuthorPublisher<S> {
    paid: PgPaidStorage,
    catalog: PgCatalog,
    registry: PgStorageRegistry,
    store: S,
    keyring: Arc<ContentKeyring>,
    bucket: String,
    bodyless_http: bool,
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
            bodyless_http: false,
        })
    }

    /// FND-00: explicit isolated S3-only paid HTTP mode. No default flip
    /// until owner-source, illustrated and scheduled browser tests are accepted.
    pub fn with_bodyless_http(mut self) -> Self {
        self.bodyless_http = true;
        self
    }
}

impl<S: PrivateObjectStore> PgPaidAuthorPublisher<S> {
    /// Seal only after the canonical paid version has been reserved by B.
    /// The keyring and plaintext live inside a bounded blocking task; if the
    /// awaiting HTTP request is cancelled, the task retains its CPU permit.
    async fn seal_in_blocking_pool(
        &self,
        version: ChapterVersionRef,
        object_id: tor_contracts::paid_delivery::ObjectId,
        markdown: &str,
    ) -> Result<EncryptedChapter, AuthorError> {
        let keyring = Arc::clone(&self.keyring);
        let markdown = zeroize::Zeroizing::new(markdown.to_owned());
        crate::cpu_work::run(move || {
            seal_chapter_with_keyring(version, object_id, &markdown, &keyring)
        })
        .await
        .map_err(|_| AuthorError::Unavailable)?
        .map_err(|_| AuthorError::Unavailable)
    }

    /// Reusable orchestration for the browser and isolated PG/S3 tests.
    pub async fn publish(&self, actor: i64, request: &SaveChapter) -> Result<(), AuthorError> {
        self.publish_mode(actor, request, false).await
    }

    /// FND-00d: opt-in, unexposed S3-only publisher. Author HTTP remains on
    /// the legacy port until private owner editor and image support are ready.
    pub async fn publish_bodyless(
        &self,
        actor: i64,
        request: &SaveChapter,
    ) -> Result<(), AuthorError> {
        self.publish_mode(actor, request, true).await
    }

    async fn publish_mode(
        &self,
        actor: i64,
        request: &SaveChapter,
        bodyless: bool,
    ) -> Result<(), AuthorError> {
        let pending = if bodyless {
            self.paid
                .prepare_new_paid_chapter_bodyless(actor, request)
                .await?
        } else {
            self.paid.prepare_new_paid_chapter(actor, request).await?
        };
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
        let sealed = self
            .seal_in_blocking_pool(version, object_id.clone(), &request.body)
            .await?;
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

        let publication = PaidPublication::Publish {
            expected_current: None,
        };
        let published = if bodyless {
            self.paid
                .attach_verified_and_publish_bodyless(
                    actor,
                    request.book_id,
                    version,
                    &object_id,
                    publication,
                    &request.body,
                )
                .await
        } else {
            self.paid
                .attach_verified_and_publish(
                    actor,
                    request.book_id,
                    version,
                    &object_id,
                    publication,
                )
                .await
        };
        if published == Err(AuthorError::Conflict) {
            // Two identical tabs can race with different random G objects.
            // Report success only when the exact idempotent B version won.
            let current = if bodyless {
                self.paid
                    .prepare_new_paid_chapter_bodyless(actor, request)
                    .await?
            } else {
                self.paid.prepare_new_paid_chapter(actor, request).await?
            };
            if current.chapter_id == pending.chapter_id
                && current.version_id == pending.version_id
                && current.published
            {
                return Ok(());
            }
        }
        published
    }

    /// S058: stage an immutable, G-verified paid edition for the book schedule.
    /// No publication pointer, release fact or reader entitlement is created.
    pub async fn stage_for_book_schedule(
        &self,
        actor: i64,
        request: &SaveChapter,
    ) -> Result<(), AuthorError> {
        self.stage_mode(actor, request, false).await
    }

    /// FND-00d: prepare a scheduled paid edition without PG plaintext.
    pub async fn stage_for_book_schedule_bodyless(
        &self,
        actor: i64,
        request: &SaveChapter,
    ) -> Result<(), AuthorError> {
        self.stage_mode(actor, request, true).await
    }

    async fn stage_mode(
        &self,
        actor: i64,
        request: &SaveChapter,
        bodyless: bool,
    ) -> Result<(), AuthorError> {
        self.paid
            .require_active_book_schedule(actor, request.book_id)
            .await?;
        let pending = if bodyless {
            self.paid
                .prepare_scheduled_paid_chapter_bodyless(actor, request)
                .await?
        } else {
            self.paid
                .prepare_scheduled_paid_chapter(actor, request)
                .await?
        };
        if pending.published {
            // The delayed command hash is distinct from the immediate path.
            // This exact scheduled edition may already have been released
            // by a worker before a retried HTTP response reaches the author.
            return Ok(());
        }
        if self
            .paid
            .staged_paid_object_attached(
                actor,
                request.book_id,
                pending.chapter_id,
                pending.version_id,
            )
            .await?
        {
            self.paid
                .rearm_prepared_paid_frontier(
                    actor,
                    request.book_id,
                    pending.chapter_id,
                    pending.version_id,
                )
                .await?;
            return Ok(());
        }
        let version = ChapterVersionRef {
            chapter_id: PositiveId::new(pending.chapter_id)
                .map_err(|_| AuthorError::Unavailable)?,
            version_id: PositiveId::new(pending.version_id)
                .map_err(|_| AuthorError::Unavailable)?,
        };
        let object_id = fresh_private_object_id().map_err(|_| AuthorError::Unavailable)?;
        let sealed = self
            .seal_in_blocking_pool(version, object_id.clone(), &request.body)
            .await?;
        let object = self
            .registry
            .reserve_pending_with_keyring(&sealed, &self.keyring, &self.bucket)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let receipt = upload_and_verify(&self.store, &object, &sealed.ciphertext)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        self.registry
            .mark_ready(&receipt)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let attached = if bodyless {
            self.paid
                .attach_verified_and_publish_bodyless(
                    actor,
                    request.book_id,
                    version,
                    &object_id,
                    PaidPublication::KeepCurrent,
                    &request.body,
                )
                .await
        } else {
            self.paid
                .attach_verified_and_publish(
                    actor,
                    request.book_id,
                    version,
                    &object_id,
                    PaidPublication::KeepCurrent,
                )
                .await
        };
        if attached == Err(AuthorError::Conflict)
            && self
                .paid
                .staged_paid_object_attached(
                    actor,
                    request.book_id,
                    pending.chapter_id,
                    pending.version_id,
                )
                .await?
        {
            self.paid
                .rearm_prepared_paid_frontier(
                    actor,
                    request.book_id,
                    pending.chapter_id,
                    pending.version_id,
                )
                .await?;
            return Ok(());
        }
        attached?;
        self.paid
            .rearm_prepared_paid_frontier(
                actor,
                request.book_id,
                pending.chapter_id,
                pending.version_id,
            )
            .await?;
        Ok(())
    }

    /// Existing paid chapter: prepare a new immutable B revision while the
    /// bought V1 remains the published and readable edition. Only after
    /// authenticated G PUT+GET/hash may B move the current pointer by CAS.
    /// This method does not expose a new HTTP route or alter paid metadata.
    pub async fn publish_edit(&self, actor: i64, request: &SaveChapter) -> Result<(), AuthorError> {
        self.publish_edit_mode(actor, request, false).await
    }

    /// FND-00d: immutable bodyless V2, preserving all historical paid V1 grants.
    pub async fn publish_edit_bodyless(
        &self,
        actor: i64,
        request: &SaveChapter,
    ) -> Result<(), AuthorError> {
        self.publish_edit_mode(actor, request, true).await
    }

    async fn publish_edit_mode(
        &self,
        actor: i64,
        request: &SaveChapter,
        bodyless: bool,
    ) -> Result<(), AuthorError> {
        let prepared = if bodyless {
            self.paid
                .prepare_published_paid_edit_bodyless(actor, request)
                .await?
        } else {
            self.paid
                .prepare_published_paid_edit(actor, request)
                .await?
        };
        if prepared.published {
            return Ok(());
        }
        let version = ChapterVersionRef {
            chapter_id: PositiveId::new(prepared.chapter_id)
                .map_err(|_| AuthorError::Unavailable)?,
            version_id: PositiveId::new(prepared.version_id)
                .map_err(|_| AuthorError::Unavailable)?,
        };
        let object_id = fresh_private_object_id().map_err(|_| AuthorError::Unavailable)?;
        let sealed = self
            .seal_in_blocking_pool(version, object_id.clone(), &request.body)
            .await?;
        let object = self
            .registry
            .reserve_pending_with_keyring(&sealed, &self.keyring, &self.bucket)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let receipt = upload_and_verify(&self.store, &object, &sealed.ciphertext)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        self.registry
            .mark_ready(&receipt)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let publication = PaidPublication::PublishEdit {
            expected_current: prepared.previous_version_id,
            expected_revision: prepared.expected_revision,
        };
        let published = if bodyless {
            self.paid
                .attach_verified_and_publish_bodyless(
                    actor,
                    request.book_id,
                    version,
                    &object_id,
                    publication,
                    &request.body,
                )
                .await
        } else {
            self.paid
                .attach_verified_and_publish(
                    actor,
                    request.book_id,
                    version,
                    &object_id,
                    publication,
                )
                .await
        };
        if published == Err(AuthorError::Conflict) {
            let existing = if bodyless {
                self.paid
                    .prepare_published_paid_edit_bodyless(actor, request)
                    .await?
            } else {
                self.paid
                    .prepare_published_paid_edit(actor, request)
                    .await?
            };
            if existing.chapter_id == prepared.chapter_id
                && existing.version_id == prepared.version_id
                && existing.published
            {
                return Ok(());
            }
        }
        published
    }
}

#[async_trait]
impl<S: PrivateObjectStore + Clone> PaidChapterPublisher for PgPaidAuthorPublisher<S> {
    async fn owned_bodyless(
        &self,
        actor: i64,
        book: i64,
        chapter: i64,
    ) -> Result<Option<OwnedChapter>, AuthorError> {
        if !self.bodyless_http {
            return Ok(None);
        }
        let Some(meta) = self
            .paid
            .owned_published_bodyless_paid_metadata(actor, book, chapter)
            .await?
        else {
            return Ok(None);
        };
        let version = ChapterVersionRef {
            chapter_id: PositiveId::new(chapter).map_err(|_| AuthorError::Invalid)?,
            version_id: PositiveId::new(meta.version_id).map_err(|_| AuthorError::Unavailable)?,
        };
        // B selects exact current paid V1/V2. G repeats live ownership,
        // object and version checks after private S3 GET/decryption.
        let Some(plain) = self
            .registry
            .owned_current_paid_markdown(
                actor,
                book,
                version,
                &self.bucket,
                &self.store,
                &self.keyring,
            )
            .await
            .map_err(|_| AuthorError::Unavailable)?
        else {
            // Never report an empty S3-only source as an editable legacy draft.
            return Err(AuthorError::Unavailable);
        };
        // A published V2 or an owner/hold change during async I/O must
        // invalidate the selected metadata before the edit form is rendered.
        if self
            .paid
            .owned_published_bodyless_paid_metadata(actor, book, chapter)
            .await?
            .as_ref()
            != Some(&meta)
        {
            return Err(AuthorError::Conflict);
        }
        Ok(Some(OwnedChapter {
            number: meta.number,
            title: meta.title,
            body: plain.as_str().to_owned(),
            access: ChapterAccess::Paid,
            price_amount: meta.price_amount,
            revision: meta.revision,
        }))
    }

    async fn publish_edit_bodyless(
        &self,
        actor: i64,
        request: &SaveChapter,
    ) -> Result<(), AuthorError> {
        if !self.bodyless_http {
            return Err(AuthorError::Unavailable);
        }
        PgPaidAuthorPublisher::publish_edit_bodyless(self, actor, request).await
    }

    async fn stage_new(&self, actor: i64, request: &SaveChapter) -> Result<(), AuthorError> {
        if self.bodyless_http {
            self.stage_for_book_schedule_bodyless(actor, request).await
        } else {
            self.stage_for_book_schedule(actor, request).await
        }
    }

    async fn publish_new(&self, actor: i64, request: &SaveChapter) -> Result<(), AuthorError> {
        if self.bodyless_http {
            self.publish_bodyless(actor, request).await
        } else {
            self.publish(actor, request).await
        }
    }

    async fn publish_edit(&self, actor: i64, request: &SaveChapter) -> Result<(), AuthorError> {
        if self.bodyless_http {
            PgPaidAuthorPublisher::publish_edit_bodyless(self, actor, request).await
        } else {
            PgPaidAuthorPublisher::publish_edit(self, actor, request).await
        }
    }

    async fn publish_with_image(
        &self,
        actor: i64,
        request: &SaveChapter,
        object_id: &tor_contracts::paid_delivery::ObjectId,
        bytes: &[u8],
        mime: &str,
    ) -> Result<(), AuthorError> {
        if self.bodyless_http {
            self.publish_illustrated_bodyless(actor, request, object_id, bytes, mime)
                .await
        } else {
            self.publish_illustrated(actor, request, object_id, bytes, mime)
                .await
        }
    }
}
