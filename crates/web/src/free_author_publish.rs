//! S349: author Save -> hidden PG IDs -> exact private S3 PUT/GET -> B publish.
//! No Markdown is ever written to PostgreSQL on this path.
use crate::author_chapter_http::FreeChapterPublisher;
use async_trait::async_trait;
use tor_books::{
    repositories::free_s3_publish::PgFreeS3Publication,
    services::authoring::{AuthorError, SaveChapter},
};
use tor_storage::{
    domain::provider::{valid_private_bucket, StorageError},
    services::free_chapter::{upload_and_verify, FreeChapterObject, FreeChapterStore},
};
mod admin;
mod import;
mod owner_pack;
mod pack_direct;

pub type S3FreeAuthor =
    PgFreeAuthorPublisher<tor_storage::repositories::free_chapter_s3::S3FreeChapterStore>;

#[derive(Clone)]
pub struct PgFreeAuthorPublisher<S> {
    repository: PgFreeS3Publication,
    store: S,
    bucket: String,
    references: tor_reading::repositories::free_s3_ref::PgFreeS3Ref,
}

impl<S: FreeChapterStore> PgFreeAuthorPublisher<S> {
    pub fn new(
        repository: PgFreeS3Publication,
        store: S,
        bucket: String,
    ) -> Result<Self, &'static str> {
        if !valid_private_bucket(&bucket) {
            return Err("Invalid private free S3 bucket");
        }
        Ok(Self {
            references: tor_reading::repositories::free_s3_ref::PgFreeS3Ref::new(
                repository.pool().clone(),
            ),
            repository,
            store,
            bucket,
        })
    }

    pub fn reader_store(&self) -> (S, String)
    where
        S: Clone,
    {
        (self.store.clone(), self.bucket.clone())
    }

    pub async fn publish(&self, actor: i64, request: &SaveChapter) -> Result<(), AuthorError> {
        self.save_new(actor, request, false).await
    }

    pub async fn save_new(
        &self,
        actor: i64,
        request: &SaveChapter,
        staged: bool,
    ) -> Result<(), AuthorError> {
        let pending = if staged {
            self.repository.prepare_staged(actor, request).await?
        } else {
            self.repository.prepare_new(actor, request).await?
        };
        if pending.published {
            return Ok(());
        }
        let object =
            FreeChapterObject::for_markdown(pending.chapter_id, pending.version_id, &request.body)
                .map_err(map_storage)?;
        let verified = upload_and_verify(&self.store, &object, &request.body)
            .await
            .map_err(map_storage)?;
        let actual = verified.object();
        let edition = (actual.chapter_id(), actual.version_id());
        let metadata = (&*self.bucket, *actual.sha256(), actual.size_bytes());
        let result = if staged {
            self.repository
                .attach_verified_and_stage(actor, request.book_id, edition, metadata)
                .await
        } else {
            self.repository
                .attach_verified_and_publish(actor, request.book_id, edition, metadata)
                .await
        };
        if result == Err(AuthorError::Conflict) {
            // A racing identical request succeeds only for this exact published edition.
            let newest = if staged {
                self.repository.prepare_staged(actor, request).await?
            } else {
                self.repository.prepare_new(actor, request).await?
            };
            if newest.chapter_id == pending.chapter_id
                && newest.version_id == pending.version_id
                && newest.published
            {
                return Ok(());
            }
        }
        result
    }
}

fn map_storage(error: StorageError) -> AuthorError {
    match error {
        StorageError::Invalid => AuthorError::Invalid,
        StorageError::Conflict => AuthorError::Conflict,
        StorageError::Corrupt | StorageError::Unavailable => AuthorError::Unavailable,
    }
}

#[async_trait]
impl<S: FreeChapterStore> FreeChapterPublisher for PgFreeAuthorPublisher<S> {
    async fn administrator_grant(
        &self,
        chapter: i64,
        version: i64,
        digest: &[u8; 32],
    ) -> Result<Option<serde_json::Value>, AuthorError> {
        self.admin_capability(chapter, version, digest).await
    }
    async fn import_job(
        &self,
        claim: &tor_imports::domain::ChapterImportClaim,
        chapters: &[tor_books::services::chapter_file_import::ImportedChapter],
    ) -> Result<usize, AuthorError> {
        self.import_queued_batch(claim, chapters).await
    }
    async fn source_for_session(
        &self,
        chapter: i64,
        version: i64,
        digest: Option<&[u8; 32]>,
    ) -> Result<Option<String>, AuthorError> {
        let record = self
            .references
            .for_session(chapter, digest)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        let Some(record) = record.filter(|r| r.version_id == version) else {
            return Ok(None);
        };
        if record.bucket != self.bucket {
            return Err(AuthorError::Unavailable);
        }
        let object =
            FreeChapterObject::from_metadata(chapter, version, record.sha256, record.size_bytes)
                .map_err(map_storage)?;
        let bytes = self
            .store
            .get_verified(&object)
            .await
            .map_err(map_storage)?;
        let current = self
            .references
            .for_session(chapter, digest)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        if current.as_ref() != Some(&record) {
            return Err(AuthorError::Missing);
        }
        String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| AuthorError::Unavailable)
    }
    async fn import_batch(
        &self,
        actor: i64,
        book: i64,
        key: &str,
        chapters: &[tor_books::services::chapter_file_import::ImportedChapter],
    ) -> Result<usize, AuthorError> {
        self.import_free_batch(actor, book, key, chapters).await
    }
    async fn publish_new(&self, actor: i64, request: &SaveChapter) -> Result<(), AuthorError> {
        self.publish(actor, request).await
    }

    async fn stage_new(&self, actor: i64, request: &SaveChapter) -> Result<(), AuthorError> {
        self.save_new(actor, request, true).await
    }

    async fn publish_edit(&self, actor: i64, request: &SaveChapter) -> Result<(), AuthorError> {
        let prepared = self.repository.prepare_edit(actor, request).await?;
        if prepared.published {
            return Ok(());
        }
        let object = FreeChapterObject::for_markdown(
            prepared.chapter_id,
            prepared.version_id,
            &request.body,
        )
        .map_err(map_storage)?;
        let verified = upload_and_verify(&self.store, &object, &request.body)
            .await
            .map_err(map_storage)?;
        let actual = verified.object();
        self.repository
            .attach_edit(
                actor,
                request,
                &prepared,
                (&self.bucket, *actual.sha256(), actual.size_bytes()),
            )
            .await
    }

    async fn owned(
        &self,
        actor: i64,
        book: i64,
        chapter: i64,
    ) -> Result<Option<tor_books::repositories::free_s3_publish::OwnedFreeS3Chapter>, AuthorError>
    {
        let Some(mut owned) = self.repository.owned(actor, book, chapter).await? else {
            return Ok(None);
        };
        if owned.bucket != self.bucket {
            return Err(AuthorError::Unavailable);
        }
        // A PACK-only import has no SINGLE object. Resolve only a verified,
        // active B placement and fetch its exact Range, then check the owner,
        // edition, revision and physical placement again after S3 I/O.
        let frame_before = if owned.stored_in_pack {
            Some(
                self.owned_pack_reference(actor, book, chapter, &owned)
                    .await?,
            )
        } else {
            None
        };
        let bytes = if let Some(frame) = frame_before.as_ref() {
            self.store.get_verified_pack(frame).await.map_err(map_storage)?
        } else {
            let object = FreeChapterObject::from_metadata(
                chapter,
                owned.version_id,
                owned.sha256,
                owned.size_bytes,
            )
            .map_err(map_storage)?;
            self.store.get_verified(&object).await.map_err(map_storage)?
        };
        let current = self
            .repository
            .owned(actor, book, chapter)
            .await?
            .ok_or(AuthorError::Missing)?;
        if current.version_id != owned.version_id
            || current.chapter.revision != owned.chapter.revision
            || current.chapter.title != owned.chapter.title
            || current.sha256 != owned.sha256
            || current.size_bytes != owned.size_bytes
            || current.bucket != owned.bucket
            || current.stored_in_pack != owned.stored_in_pack
        {
            return Err(AuthorError::Conflict);
        }
        if let Some(before) = frame_before {
            let after = self
                .owned_pack_reference(actor, book, chapter, &current)
                .await?;
            if after != before {
                return Err(AuthorError::Conflict);
            }
        }
        owned.chapter.body = String::from_utf8(bytes).map_err(|_| AuthorError::Unavailable)?;
        Ok(Some(owned))
    }
}
