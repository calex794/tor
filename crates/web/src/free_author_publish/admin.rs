//! Privileged free grants use the existing audited administrator POST routes.
use super::*;
use axum::http::Uri;
use std::time::Duration;

impl<S: FreeChapterStore> PgFreeAuthorPublisher<S> {
    pub(super) async fn admin_capability(
        &self,
        chapter: i64,
        version: i64,
        digest: &[u8; 32],
    ) -> Result<Option<serde_json::Value>, AuthorError> {
        let Some(record) = self
            .references
            .for_administrator(chapter, version, digest)
            .await
            .map_err(|_| AuthorError::Unavailable)?
        else {
            return Ok(None);
        };
        if record.bucket != self.bucket {
            return Err(AuthorError::Unavailable);
        }
        let object =
            FreeChapterObject::from_metadata(chapter, version, record.sha256, record.size_bytes)
                .map_err(map_storage)?;
        let signed = self
            .store
            .presign_verified(&object, Duration::from_secs(90))
            .await
            .map_err(map_storage)?;
        let value = serde_json::to_value(&signed).map_err(|_| AuthorError::Unavailable)?;
        let url: Uri = value
            .as_str()
            .ok_or(AuthorError::Unavailable)?
            .parse()
            .map_err(|_| AuthorError::Unavailable)?;
        let origin =
            std::env::var("TOR_FREE_S3_PUBLIC_ORIGIN").map_err(|_| AuthorError::Unavailable)?;
        if !crate::paid_grants_http::valid_reader_s3_origin(&origin) {
            return Err(AuthorError::Unavailable);
        }
        let origin: Uri = origin.parse().map_err(|_| AuthorError::Unavailable)?;
        if url.scheme() != origin.scheme() || url.authority() != origin.authority() {
            return Err(AuthorError::Unavailable);
        }
        let current = self
            .references
            .for_administrator(chapter, version, digest)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
        if current.as_ref() != Some(&record) {
            return Err(AuthorError::Forbidden);
        }
        Ok(Some(serde_json::json!({
            "schema":"tor.free-s3-grant.v1", "chapter_id":chapter.to_string(),
            "version_id":version.to_string(), "url":signed,
            "sha256":record.sha256.iter().map(|b|format!("{b:02x}")).collect::<String>(),
            "size_bytes":record.size_bytes,
        })))
    }
}
