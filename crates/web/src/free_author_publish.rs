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
mod import;

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
        let object = FreeChapterObject::from_metadata(
            chapter,
            owned.version_id,
            owned.sha256,
            owned.size_bytes,
        )
        .map_err(map_storage)?;
        let bytes = self
            .store
            .get_verified(&object)
            .await
            .map_err(map_storage)?;
        let current = self
            .repository
            .owned(actor, book, chapter)
            .await?
            .ok_or(AuthorError::Missing)?;
        if current.version_id != owned.version_id
            || current.chapter.revision != owned.chapter.revision
        {
            return Err(AuthorError::Conflict);
        }
        owned.chapter.body = String::from_utf8(bytes).map_err(|_| AuthorError::Unavailable)?;
        Ok(Some(owned))
    }
}
