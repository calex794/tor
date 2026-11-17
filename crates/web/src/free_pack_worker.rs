//! DROP-R4: trusted, offline S3 proof -> B transaction coordinator.
//! No route, no background startup and no active reader cutover.
//! The B-approved offline pack builder/writer is never exposed to HTTP actors.

pub mod activation;
pub mod runner;
use tor_books::repositories::{free_pack_attach::PgFreePackCatalog, free_pack_jobs::PackJobClaim};
use tor_books::services::authoring::AuthorError;
use tor_storage::repositories::free_pack_proof_s3::S3PackProofStore;
use tor_storage::services::{free_chapter::FreeChapterObject, free_pack_proof::PackProofTarget};

async fn reverify_pack_receipt(
    proof: &S3PackProofStore,
    claim: &PackJobClaim,
) -> Result<
    tor_contracts::drop_pack::PackPlacementReceipt,
    tor_storage::domain::provider::StorageError,
> {
    let approved = &claim.receipt;
    let source = approved
        .frames
        .iter()
        .map(|frame| {
            FreeChapterObject::from_metadata(
                frame.chapter_id,
                frame.version_id,
                frame.content_sha256,
                frame.size_bytes,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let target = PackProofTarget {
        bucket: approved.target.bucket.clone(),
        pack_key: approved.target.pack_key.clone(),
        manifest_key: approved.target.manifest_key.clone(),
        pack_sha256: approved.target.pack_sha256,
        manifest_sha256: approved.target.manifest_sha256,
        pack_bytes: approved.target.pack_bytes,
        manifest_bytes: approved.target.manifest_bytes,
        seek_table_start: approved.target.seek_table_start,
    };
    let verified = proof.verify_independent_full_get(&target, &source).await?;
    Ok(verified.placement_receipt())
}

pub async fn finalize_one_verified_pack(
    catalog: &PgFreePackCatalog,
    proof: &S3PackProofStore,
    claim: &PackJobClaim,
) -> Result<i64, AuthorError> {
    let receipt = reverify_pack_receipt(proof, claim)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
    if receipt != claim.receipt {
        return Err(AuthorError::Conflict);
    }
    catalog
        .finish_pack_job(claim.job_id, claim.epoch, &receipt)
        .await
}

/// R4b2: keep the B-approved immutable descriptor fixed across the complete
/// write/verify/commit sequence. No application PG transaction is held during
/// remote I/O; the current lease is rechecked atomically by B at the end.
pub async fn upload_one_approved_pack(
    catalog: &PgFreePackCatalog,
    writer: &tor_storage::repositories::free_pack_write_s3::S3FreePackWriter,
    claim: &PackJobClaim,
    prepared: &tor_storage::services::free_pack_write::PreparedFreePack,
) -> Result<i64, AuthorError> {
    if *prepared.receipt() != claim.receipt {
        return Err(AuthorError::Conflict);
    }
    let verified = writer
        .upload_and_verify(prepared)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
    catalog
        .finish_pack_job(claim.job_id, claim.epoch, &verified.placement_receipt())
        .await
}

/// DROP-05a: internal-only, independently reverified activation of an
/// already succeeded candidate. Never called by HTTP or a startup loop.
/// The full G proof and B all-member CAS are deliberately separate; failure
/// at either boundary leaves every original S349 SINGLE intact.
async fn reprove_pack_for_cutover(
    catalog: &PgFreePackCatalog,
    proof: &S3PackProofStore,
    original_single: &impl tor_storage::services::free_chapter::FreeChapterStore,
    claim: &PackJobClaim,
) -> Result<(), AuthorError> {
    // The original SINGLE is the recoverable reserve. Every source is
    // checked with bounded private GET, not assumed from immutable B metadata.
    for frame in &claim.receipt.frames {
        let exact = FreeChapterObject::from_metadata(
            frame.chapter_id,
            frame.version_id,
            frame.content_sha256,
            frame.size_bytes,
        )
        .map_err(|_| AuthorError::Invalid)?;
        original_single
            .get_verified(&exact)
            .await
            .map_err(|error| match error {
                tor_storage::domain::provider::StorageError::Unavailable => {
                    AuthorError::Unavailable
                }
                _ => AuthorError::Conflict,
            })?;
    }
    // A succeeded R4a job is an idempotent readback, not permission to reuse
    // an old proof: fetch full PACK and manifest and validate each frame now.
    // A corrupt immutable PACK is terminal; a transient S3 outage is retryable.
    let receipt = reverify_pack_receipt(proof, claim)
        .await
        .map_err(|error| match error {
            tor_storage::domain::provider::StorageError::Unavailable => AuthorError::Unavailable,
            _ => AuthorError::Conflict,
        })?;
    if receipt != claim.receipt {
        return Err(AuthorError::Conflict);
    }
    catalog
        .finish_pack_job(claim.job_id, claim.epoch, &receipt)
        .await?;
    Ok(())
}

/// DROP-05a manual trusted call; does not consume an activation queue lease.
pub async fn activate_one_verified_pack(
    catalog: &PgFreePackCatalog,
    proof: &S3PackProofStore,
    original_single: &impl tor_storage::services::free_chapter::FreeChapterStore,
    claim: &PackJobClaim,
) -> Result<tor_books::repositories::free_pack_cutover::PackActivation, AuthorError> {
    reprove_pack_for_cutover(catalog, proof, original_single, claim).await?;
    catalog
        .activate_verified_pack(claim.job_id, claim.epoch, &claim.receipt)
        .await
}

/// DROP-05b: trusted queued worker must use the *separate* activation epoch
/// in the same B transaction as the all-member PACK pointer CAS.
pub async fn activate_one_queued_verified_pack(
    catalog: &PgFreePackCatalog,
    proof: &S3PackProofStore,
    original_single: &impl tor_storage::services::free_chapter::FreeChapterStore,
    claim: &tor_books::repositories::free_pack_activation::PackActivationClaim,
) -> Result<tor_books::repositories::free_pack_cutover::PackActivation, AuthorError> {
    reprove_pack_for_cutover(catalog, proof, original_single, &claim.pack).await?;
    catalog.activate_queued_verified_pack(claim).await
}

/// DROP-05c: explicit operator-only physical recovery. A corrupt or missing
/// active PACK is NOT a reason for automatic fallback. An operator must first
/// independently verify ALL original immutable SINGLE bytes, then B atomically
/// creates newer SINGLE generations for the complete approved PACK membership.
/// This function is not wired to an HTTP route, startup or automatic worker.
pub async fn rollback_one_verified_pack(
    catalog: &PgFreePackCatalog,
    original_single: &impl tor_storage::services::free_chapter::FreeChapterStore,
    claim: &PackJobClaim,
) -> Result<tor_books::repositories::free_pack_rollback::PackRollback, AuthorError> {
    for frame in &claim.receipt.frames {
        let object = FreeChapterObject::from_metadata(
            frame.chapter_id,
            frame.version_id,
            frame.content_sha256,
            frame.size_bytes,
        )
        .map_err(|_| AuthorError::Invalid)?;
        original_single
            .get_verified(&object)
            .await
            .map_err(|_| AuthorError::Unavailable)?;
    }
    catalog
        .rollback_verified_pack_to_single(claim.job_id, claim.epoch, &claim.receipt)
        .await
}
