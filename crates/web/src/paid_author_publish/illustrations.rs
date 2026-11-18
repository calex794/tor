//! FND-00h: illustrated author publication with an explicit bodyless mode.
//! Both variants bind the server-minted asset to the exact immutable B version.
//! G verifies the private upload before B makes the chapter visible.
use super::*;

impl<S: PrivateObjectStore + Clone> PgPaidAuthorPublisher<S> {
    /// Compatibility for retained legacy published content and existing tests.
    pub async fn publish_illustrated(
        &self,
        actor: i64,
        request: &SaveChapter,
        object_id: &tor_contracts::paid_delivery::ObjectId,
        bytes: &[u8],
        mime: &str,
    ) -> Result<(), AuthorError> {
        self.publish_illustrated_mode(actor, request, object_id, bytes, mime, false)
            .await
    }

    /// Active only under the explicit operator-controlled S3-only HTTP mode.
    pub(crate) async fn publish_illustrated_bodyless(
        &self,
        actor: i64,
        request: &SaveChapter,
        object_id: &tor_contracts::paid_delivery::ObjectId,
        bytes: &[u8],
        mime: &str,
    ) -> Result<(), AuthorError> {
        self.publish_illustrated_mode(actor, request, object_id, bytes, mime, true)
            .await
    }

    async fn publish_illustrated_mode(
        &self,
        actor: i64,
        request: &SaveChapter,
        object_id: &tor_contracts::paid_delivery::ObjectId,
        bytes: &[u8],
        mime: &str,
        bodyless: bool,
    ) -> Result<(), AuthorError> {
        if actor <= 0 || stable_image_id(actor, request.book_id, &request.command_key) != *object_id
        {
            return Err(AuthorError::Invalid);
        }
        let pending = if bodyless {
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
            // A lost HTTP response is a true replay only for exactly the
            // already verified image, command, version and digest.
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
        if bodyless {
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
