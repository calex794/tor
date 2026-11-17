//! DROP-R4: trusted, offline S3 proof -> B transaction coordinator.
//! No route, no background startup and no active reader cutover.
//! The B-approved offline pack builder/writer is never exposed to HTTP actors.

pub mod runner;
use tor_books::repositories::{free_pack_attach::PgFreePackCatalog, free_pack_jobs::PackJobClaim};
use tor_books::services::authoring::AuthorError;
use tor_storage::repositories::free_pack_proof_s3::S3PackProofStore;
use tor_storage::services::{free_chapter::FreeChapterObject, free_pack_proof::PackProofTarget};

pub async fn finalize_one_verified_pack(
    catalog: &PgFreePackCatalog,
    proof: &S3PackProofStore,
    claim: &PackJobClaim,
) -> Result<i64, AuthorError> {
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
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| AuthorError::Invalid)?;
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
    // The only path into the B finalizer is an independent real S3 readback.
    let verified = proof
        .verify_independent_full_get(&target, &source)
        .await
        .map_err(|_| AuthorError::Unavailable)?;
    let receipt = verified.placement_receipt();
    if receipt != *approved {
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
pub async fn activate_one_verified_pack(
    catalog: &PgFreePackCatalog,
    proof: &S3PackProofStore,
    claim: &PackJobClaim,
) -> Result<tor_books::repositories::free_pack_cutover::PackActivation, AuthorError> {
    // Reuse the exact R4b1 proof path, including full PACK + manifest readback,
    // manifest/frame matching and the immutable approved receipt comparison.
    // For a succeeded R4a job finish_pack_job is an idempotent readback;
    // it must not create a second physical generation.
    finalize_one_verified_pack(catalog, proof, claim).await?;
    catalog
        .activate_verified_pack(claim.job_id, claim.epoch, &claim.receipt)
        .await
}
