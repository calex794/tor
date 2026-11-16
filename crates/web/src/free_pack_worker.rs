//! DROP-R4b1: trusted, offline S3 proof -> B transaction coordinator.
//! No route, no background startup, no upload and no active reader cutover.
//! A separate pack builder must upload immutable objects before scheduling.
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
