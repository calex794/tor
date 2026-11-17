//! DROP-04c: owner-only PACK source for editing a chapter imported without SINGLE.
//! B proves the active immutable generation; G fetches exactly one bounded frame.
use super::*;
use tor_books::{
    repositories::{free_pack_attach::PgFreePackCatalog, free_s3_publish::OwnedFreeS3Chapter},
    services::authoring::AuthorError,
};
use tor_storage::services::free_placement::{PackMetadata, ValidatedPackReference};

impl<S: FreeChapterStore> PgFreeAuthorPublisher<S> {
    pub(super) async fn owned_pack_reference(
        &self,
        actor: i64,
        book: i64,
        chapter: i64,
        owned: &OwnedFreeS3Chapter,
    ) -> Result<ValidatedPackReference, AuthorError> {
        if !owned.stored_in_pack || owned.bucket != self.bucket {
            return Err(AuthorError::Unavailable);
        }
        // The metadata is never a grant: the caller has independently
        // resolved and checked the exact live owner and revision in B.
        let fresh = self
            .repository
            .owned(actor, book, chapter)
            .await?
            .ok_or(AuthorError::Missing)?;
        if !fresh.stored_in_pack
            || fresh.version_id != owned.version_id
            || fresh.chapter.revision != owned.chapter.revision
            || fresh.sha256 != owned.sha256
            || fresh.size_bytes != owned.size_bytes
            || fresh.bucket != owned.bucket
        {
            return Err(AuthorError::Conflict);
        }
        let record = PgFreePackCatalog::new(self.repository.pool().clone())
            .active_verified_free_range(
                chapter,
                owned.version_id,
                owned.sha256,
                owned.size_bytes,
                &owned.bucket,
            )
            .await?
            .ok_or(AuthorError::Unavailable)?;
        let edition = FreeChapterObject::from_metadata(
            chapter,
            owned.version_id,
            owned.sha256,
            owned.size_bytes,
        )
        .map_err(map_storage)?;
        ValidatedPackReference::new(
            edition,
            PackMetadata {
                bucket: record.bucket,
                key: record.key,
                pack_bytes: record.pack_bytes,
                seek_table_start: record.seek_table_start,
                frame_offset: record.frame_offset,
                frame_bytes: record.frame_bytes,
                pack_sha256: record.pack_sha256,
                manifest_sha256: record.manifest_sha256,
            },
        )
        .map_err(map_storage)
    }
}
