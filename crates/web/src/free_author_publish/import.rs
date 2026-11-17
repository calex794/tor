//! B reserves hidden IDs; G verifies all objects; B commits the whole batch.
use super::*;
use tor_books::{
    repositories::free_s3_publish::{
        FreeImportTransport, ImportTransportRequest, VerifiedFreeImportEdition,
    },
    services::chapter_file_import::{ImportedChapter, MAX_IMPORT_CHAPTERS},
};

impl<S: FreeChapterStore> PgFreeAuthorPublisher<S> {
    pub async fn import_free_batch(
        &self,
        actor: i64,
        book: i64,
        key: &str,
        chapters: &[ImportedChapter],
    ) -> Result<usize, AuthorError> {
        let editions = self.prepare_import(actor, book, key, chapters).await?;
        self.repository
            .finish_import(actor, book, key, chapters, &self.bucket, &editions)
            .await
    }

    pub async fn import_queued_batch(
        &self,
        claim: &tor_imports::domain::ChapterImportClaim,
        chapters: &[ImportedChapter],
    ) -> Result<usize, AuthorError> {
        if self
            .repository
            .select_import_transport(ImportTransportRequest {
                job: (claim.job_id, claim.epoch),
                actor: claim.actor_id,
                book: claim.book_id,
                command_key: &claim.command_key,
                chapters,
                prefer_pack: false,
            })
            .await?
            != FreeImportTransport::Single
        {
            return Err(AuthorError::Conflict);
        }
        let editions = self
            .prepare_import(claim.actor_id, claim.book_id, &claim.command_key, chapters)
            .await?;
        self.repository
            .finish_import_job(
                (claim.job_id, claim.epoch),
                chapters,
                &self.bucket,
                &editions,
            )
            .await
    }

    async fn prepare_import(
        &self,
        actor: i64,
        book: i64,
        key: &str,
        chapters: &[ImportedChapter],
    ) -> Result<Vec<VerifiedFreeImportEdition>, AuthorError> {
        if chapters.is_empty()
            || chapters.len() > MAX_IMPORT_CHAPTERS
            || !tor_books::services::authoring::valid_command_key(key)
        {
            return Err(AuthorError::Invalid);
        }
        let pending = self
            .repository
            .prepare_import(actor, book, key, chapters)
            .await?;
        let destinations = chapters
            .iter()
            .zip(pending)
            .map(|(chapter, pending)| {
                FreeChapterObject::for_markdown(
                    pending.chapter_id,
                    pending.version_id,
                    &chapter.body,
                )
                .map(|object| (chapter, pending, object))
                .map_err(map_storage)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let keys: Vec<_> = destinations
            .iter()
            .map(|(_, _, object)| object.key())
            .collect();
        let keys: Vec<_> = keys.iter().map(String::as_str).collect();
        self.reserve_s3_keys(&keys).await?;
        let mut editions = Vec::with_capacity(chapters.len());
        for (chapter, pending, object) in destinations {
            if pending.verified {
                // A durable checkpoint is not proof that the object still
                // exists. Recheck bytes before committing any live pointer.
                self.store
                    .get_verified(&object)
                    .await
                    .map_err(map_storage)?;
            } else {
                upload_and_verify(&self.store, &object, &chapter.body)
                    .await
                    .map_err(map_storage)?;
            }
            let edition = VerifiedFreeImportEdition {
                chapter_id: object.chapter_id(),
                version_id: object.version_id(),
                sha256: *object.sha256(),
                size_bytes: object.size_bytes(),
            };
            self.repository
                .mark_import_verified(actor, key, &self.bucket, &edition)
                .await?;
            editions.push(edition);
        }
        Ok(editions)
    }
}
