//! Internal verified SINGLE/PACK source -> exact B preparation.
//! No public URL, reader permission or acknowledgement is created here.
use super::*;
use sha2::{Digest, Sha256};
use tor_storage::{
    domain::provider::valid_private_bucket,
    services::{
        free_chapter::{FreeChapterObject, FreeChapterStore},
        free_pack_read::PackReadDeadline,
        free_placement::{PackMetadata, ValidatedPackReference},
    },
};

/// One bounded attempt. Start the shared monotonic deadline before PG
/// selection, not just before S3. Retry/ack remains with the G consumer.
pub async fn prepare_verified_job<S: FreeChapterStore>(
    repo: &PgGuestPrefix,
    job: &GuestPrefixJob,
    store: &S,
    trusted_bucket: &str,
) -> Result<Option<GuestPrefixPreparation>, AuthorError> {
    let deadline = PackReadDeadline::start();
    let selected = deadline
        .wait(repo.claimed_source(job))
        .await
        .map_err(|_| AuthorError::Unavailable)??;
    let Some(selected) = selected else {
        return Ok(None);
    };
    let stored = match &selected {
        GuestPrefixSource::Legacy(source) => {
            return deadline
                .wait(repo.prepare_claimed(job, source))
                .await
                .map_err(|_| AuthorError::Unavailable)?
                .map(Some);
        }
        GuestPrefixSource::Single(stored)
        | GuestPrefixSource::Pack {
            edition: stored, ..
        } => stored,
    };
    if !valid_private_bucket(trusted_bucket) || stored.bucket != trusted_bucket {
        return Err(AuthorError::Unavailable);
    }
    let target = job.target();
    let edition = FreeChapterObject::from_metadata(
        target.chapter_id,
        target.version_id,
        stored.sha256,
        stored.size_bytes,
    )
    .map_err(|_| AuthorError::Unavailable)?;
    let bytes = match &selected {
        GuestPrefixSource::Single(_) => deadline
            .wait(store.get_verified(&edition))
            .await
            .map_err(|_| AuthorError::Unavailable)?
            .map_err(|_| AuthorError::Unavailable)?,
        GuestPrefixSource::Pack { frame, .. } => {
            if frame.bucket != trusted_bucket {
                return Err(AuthorError::Unavailable);
            }
            let frame = ValidatedPackReference::new(
                edition,
                PackMetadata {
                    bucket: frame.bucket.clone(),
                    key: frame.key.clone(),
                    pack_bytes: frame.pack_bytes,
                    seek_table_start: frame.seek_table_start,
                    frame_offset: frame.frame_offset,
                    frame_bytes: frame.frame_bytes,
                    pack_sha256: frame.pack_sha256,
                    manifest_sha256: frame.manifest_sha256,
                },
            )
            .map_err(|_| AuthorError::Unavailable)?;
            deadline
                .wait(store.get_verified_pack(&frame))
                .await
                .map_err(|_| AuthorError::Unavailable)?
                .map_err(|_| AuthorError::Unavailable)?
        }
        GuestPrefixSource::Legacy(_) => return Err(AuthorError::Unavailable),
    };
    // The provider port is not a substitute for matching B's exact receipt.
    let actual: [u8; 32] = Sha256::digest(&bytes).into();
    if bytes.len() as u64 != stored.size_bytes || actual != stored.sha256 {
        return Err(AuthorError::Conflict);
    }
    let source = std::str::from_utf8(&bytes).map_err(|_| AuthorError::Unavailable)?;
    let fresh = deadline
        .wait(repo.claimed_source(job))
        .await
        .map_err(|_| AuthorError::Unavailable)??;
    let Some(fresh) = fresh else {
        return Ok(None);
    };
    if fresh != selected {
        return Err(AuthorError::Conflict);
    }
    // A changed policy, publication, lease or source is rejected even
    // between this lookup and the final B transaction.
    deadline
        .wait(repo.prepare_claimed(job, source))
        .await
        .map_err(|_| AuthorError::Unavailable)?
        .map(Some)
}
