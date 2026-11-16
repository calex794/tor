//! Original DROP-04a: direct packed upload from a S023 approved batch.
//! No per-chapter SINGLE upload, no reader publication, no chapter INSERT.
//! The next accepted DROP-04b must atomically bind the verified staged proof
//! to a live edition and suitable C-authorized Range reader.
use super::*;
use sha2::{Digest, Sha256};
use tor_books::{
    repositories::free_s3_publish::{DirectPackFinalize, DirectPackStageRequest},
    services::chapter_file_import::{fingerprint, ImportedChapter, MAX_IMPORT_CHAPTERS},
};
use tor_contracts::drop_pack::PackPlacementReceipt;
use tor_storage::{
    repositories::{free_pack_proof_s3::S3PackProofStore, free_pack_write_s3::S3FreePackWriter},
    services::{free_pack_proof::PackProofTarget, free_pack_write::prepare_free_pack},
};

impl<S: FreeChapterStore> PgFreeAuthorPublisher<S> {
    /// This offline entrypoint is NOT called by the existing live import
    /// worker. It can only stage a verified result; the old S349 route stays
    /// unchanged until finalizer and reader acceptance are complete.
    pub async fn stage_direct_pack_only(
        &self,
        claim: &tor_imports::domain::ChapterImportClaim,
        chapters: &[ImportedChapter],
        writer: &S3FreePackWriter,
    ) -> Result<PackPlacementReceipt, AuthorError> {
        if claim.job_id <= 0
            || claim.epoch <= 0
            || claim.actor_id <= 0
            || claim.book_id <= 0
            || chapters.is_empty()
            || chapters.len() > MAX_IMPORT_CHAPTERS
            || chapters.len() != claim.expected_chapter_count as usize
            || claim.approved_fingerprint.as_slice()
                != fingerprint(claim.book_id, chapters).as_slice()
        {
            return Err(AuthorError::Conflict);
        }
        // B reserves the exact IDs and UTF-16 shapes without writing public
        // chapters. Failed/expired uploads leave only inert reservations.
        let pending = self
            .repository
            .prepare_import(claim.actor_id, claim.book_id, &claim.command_key, chapters)
            .await?;
        if pending.len() != chapters.len() || pending.iter().any(|p| p.verified) {
            // Never mix a partially uploaded SINGLE import with direct PACK.
            return Err(AuthorError::Conflict);
        }
        let mut sources = Vec::with_capacity(chapters.len());
        for (chapter, prepared) in chapters.iter().zip(pending) {
            let edition = FreeChapterObject::for_markdown(
                prepared.chapter_id,
                prepared.version_id,
                &chapter.body,
            )
            .map_err(map_storage)?;
            sources.push((edition, chapter.body.as_bytes().to_vec()));
        }
        let key = direct_pack_key(
            claim.book_id,
            claim.actor_id,
            &claim.command_key,
            fingerprint(claim.book_id, chapters),
        );
        let bucket = self.bucket.clone();
        let packed =
            tokio::task::spawn_blocking(move || prepare_free_pack(&bucket, &key, &sources))
                .await
                .map_err(|_| AuthorError::Unavailable)?
                .map_err(map_storage)?;
        let proof = writer
            .upload_and_verify(&packed)
            .await
            .map_err(map_storage)?;
        let receipt = proof.placement_receipt();
        if receipt != *packed.receipt() {
            return Err(AuthorError::Conflict);
        }
        self.repository
            .stage_verified_direct_pack(DirectPackStageRequest {
                job_id: claim.job_id,
                epoch: claim.epoch,
                actor: claim.actor_id,
                book: claim.book_id,
                command_key: &claim.command_key,
                chapters,
                expected_bucket: &self.bucket,
                receipt: &receipt,
            })
            .await?;
        Ok(receipt)
    }
}

/// Complete the original DROP-04b batch only after a fresh, independently
/// verified full S3 GET. This is never called from the normal live importer
/// until a C-authorized HTTP PACK reader is accepted and feature-gated.
impl<S: FreeChapterStore> PgFreeAuthorPublisher<S> {
    pub async fn finalize_direct_pack_after_reproof(
        &self,
        claim: &tor_imports::domain::ChapterImportClaim,
        chapters: &[ImportedChapter],
        receipt: &PackPlacementReceipt,
        proof: &S3PackProofStore,
    ) -> Result<usize, AuthorError> {
        if claim.job_id <= 0
            || claim.epoch <= 0
            || claim.book_id <= 0
            || chapters.len() != receipt.frames.len()
            || chapters.is_empty()
            || chapters.len() > MAX_IMPORT_CHAPTERS
            || claim.approved_fingerprint.as_slice()
                != fingerprint(claim.book_id, chapters).as_slice()
            || receipt.target.bucket != self.bucket
        {
            return Err(AuthorError::Conflict);
        }
        let mut expected = Vec::with_capacity(chapters.len());
        for (frame, chapter) in receipt.frames.iter().zip(chapters) {
            let edition =
                FreeChapterObject::for_markdown(frame.chapter_id, frame.version_id, &chapter.body)
                    .map_err(map_storage)?;
            if frame.content_sha256 != *edition.sha256() || frame.size_bytes != edition.size_bytes()
            {
                return Err(AuthorError::Conflict);
            }
            expected.push(edition);
        }
        let target = &receipt.target;
        let proven = proof
            .verify_independent_full_get(
                &PackProofTarget {
                    bucket: target.bucket.clone(),
                    pack_key: target.pack_key.clone(),
                    manifest_key: target.manifest_key.clone(),
                    pack_sha256: target.pack_sha256,
                    manifest_sha256: target.manifest_sha256,
                    pack_bytes: target.pack_bytes,
                    manifest_bytes: target.manifest_bytes,
                    seek_table_start: target.seek_table_start,
                },
                &expected,
            )
            .await
            .map_err(map_storage)?;
        if proven.placement_receipt() != *receipt {
            return Err(AuthorError::Conflict);
        }
        // All physical bytes are proven again. The only authority to
        // materialize reader metadata is B's fenced PostgreSQL transaction.
        self.repository
            .finish_verified_direct_pack_job(DirectPackFinalize {
                job_id: claim.job_id,
                epoch: claim.epoch,
                actor: claim.actor_id,
                book: claim.book_id,
                command_key: &claim.command_key,
                chapters,
                receipt,
            })
            .await
    }
}

fn direct_pack_key(book: i64, actor: i64, command_key: &str, digest: [u8; 32]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"tor-drop-04-direct-import-v1");
    hasher.update(book.to_be_bytes());
    hasher.update(actor.to_be_bytes());
    hasher.update(command_key.as_bytes());
    hasher.update(digest);
    let id: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("private/free-packs/import/{book}/{id}.zst")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_approved_batch_has_one_immutable_destination() {
        let a = direct_pack_key(5, 7, &"g".repeat(43), [9; 32]);
        assert!(a.starts_with("private/free-packs/import/5/"));
        assert_eq!(a, direct_pack_key(5, 7, &"g".repeat(43), [9; 32]));
        assert_ne!(a, direct_pack_key(5, 8, &"g".repeat(43), [9; 32]));
        assert_ne!(a, direct_pack_key(5, 7, &"g".repeat(43), [8; 32]));
        assert!(a.ends_with(".zst"));
    }
}
