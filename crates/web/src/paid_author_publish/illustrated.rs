//! FND-00: one-image paid author form, legacy and bodyless B/G modes.
use super::*;
use crate::author_chapter_images::stable_image_id;
use crate::paid_author_assets::PgPaidAuthorAssets;

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
        if actor <= 0 || stable_image_id(actor, request.book_id, &request.command_key) != *object_id
        {
            return Err(AuthorError::Invalid);
        }
        let pending = if self.bodyless_http {
            self.paid
                .prepare_new_paid_chapter_bodyless(actor, request)
                .await?
        } else {
            self.paid.prepare_new_paid_chapter(actor, request).await?
        };
        let pinned = ChapterVersionRef {
            chapter_id: PositiveId::new(pending.chapter_id)
                .map_err(|_| AuthorError::Unavailable)?,
            version_id: PositiveId::new(pending.version_id)
                .map_err(|_| AuthorError::Unavailable)?,
        };
        if pending.published {
            // Lost HTTP response is a true retry ONLY for identical bytes.
            let digest = tor_storage::services::crypto::sha256_bytes(bytes);
            return match tor_books::repositories::paid_assets::PgPaidAssets::new(
                self.catalog.pool(),
            )
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
        if self.bodyless_http {
            uploader
                .upload_to_draft_with_id_bodyless(actor, request, 1, bytes, mime, object_id)
                .await?;
            self.publish_bodyless(actor, request).await
        } else {
            uploader
                .upload_to_draft_with_id(actor, request, 1, bytes, mime, object_id)
                .await?;
            self.publish(actor, request).await
        }
    }
}
