//! Paid private-storage publication port used by authoring HTTP.
use async_trait::async_trait;
use tor_books::services::authoring::{AuthorError, SaveChapter};

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
