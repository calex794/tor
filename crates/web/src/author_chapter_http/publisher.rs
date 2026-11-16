//! Paid private-storage publication port used by authoring HTTP.
use async_trait::async_trait;
use tor_books::services::authoring::{AuthorError, SaveChapter};

#[derive(Clone)]
pub(crate) struct FreeContent(pub std::sync::Arc<dyn FreeChapterPublisher>);

/// Only the S3-paid constructor provides this port. Ordinary author routes
/// remain entirely unable to write private paid content.
#[async_trait]
pub(crate) trait PaidChapterPublisher: Send + Sync {
    async fn publish_new(&self, actor: i64, request: &SaveChapter) -> Result<(), AuthorError>;

    async fn stage_new(&self, _actor: i64, _request: &SaveChapter) -> Result<(), AuthorError> {
        Err(AuthorError::Unavailable)
    }

    /// No paid edit without a separately configured, verified S3 writer.
    async fn publish_edit(&self, _actor: i64, _request: &SaveChapter) -> Result<(), AuthorError> {
        Err(AuthorError::Unavailable)
    }

    /// The isolated illustrated-author route supplies a server-computed
    /// immutable image identity. Non-S3 publishers fail closed by default.
    async fn publish_with_image(
        &self,
        _actor: i64,
        _request: &SaveChapter,
        _object_id: &tor_contracts::paid_delivery::ObjectId,
        _bytes: &[u8],
        _mime: &str,
    ) -> Result<(), AuthorError> {
        Err(AuthorError::Unavailable)
    }
}

#[async_trait]
pub(crate) trait FreeChapterPublisher: Send + Sync {
    async fn administrator_grant(
        &self,
        chapter: i64,
        version: i64,
        digest: &[u8; 32],
    ) -> Result<Option<serde_json::Value>, AuthorError>;
    /// Public-book Save is available only after a verified S3 PUT/GET.
    async fn publish_new(&self, actor: i64, request: &SaveChapter) -> Result<(), AuthorError>;

    async fn stage_new(&self, actor: i64, request: &SaveChapter) -> Result<(), AuthorError>;
    async fn publish_edit(&self, actor: i64, request: &SaveChapter) -> Result<(), AuthorError>;
    async fn owned(
        &self,
        actor: i64,
        book: i64,
        chapter: i64,
    ) -> Result<Option<tor_books::repositories::free_s3_publish::OwnedFreeS3Chapter>, AuthorError>;

    async fn import_batch(
        &self,
        actor: i64,
        book: i64,
        key: &str,
        chapters: &[tor_books::services::chapter_file_import::ImportedChapter],
    ) -> Result<usize, AuthorError>;

    async fn source_for_session(
        &self,
        chapter: i64,
        version: i64,
        digest: Option<&[u8; 32]>,
    ) -> Result<Option<String>, AuthorError>;

    async fn import_job(
        &self,
        claim: &tor_imports::domain::ChapterImportClaim,
        chapters: &[tor_books::services::chapter_file_import::ImportedChapter],
    ) -> Result<usize, AuthorError>;
}
