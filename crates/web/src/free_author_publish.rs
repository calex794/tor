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

pub struct PgFreeAuthorPublisher<S> {
    repository: PgFreeS3Publication,
    store: S,
    bucket: String,
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
            repository,
            store,
            bucket,
        })
    }

    pub async fn publish(&self, actor: i64, request: &SaveChapter) -> Result<(), AuthorError> {
        let pending = self.repository.prepare_new(actor, request).await?;
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
        let result = self
            .repository
            .attach_verified_and_publish(
                actor,
                request.book_id,
                (actual.chapter_id(), actual.version_id()),
                (&self.bucket, *actual.sha256(), actual.size_bytes()),
            )
            .await;
        if result == Err(AuthorError::Conflict) {
            // Identical retries can race with another matching upload.
            let newest = self.repository.prepare_new(actor, request).await?;
            if newest == pending
                || (newest.chapter_id == pending.chapter_id
                    && newest.version_id == pending.version_id
                    && newest.published)
            {
                if newest.published {
                    return Ok(());
                }
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
    async fn publish_new(&self, actor: i64, request: &SaveChapter) -> Result<(), AuthorError> {
        self.publish(actor, request).await
    }
}
