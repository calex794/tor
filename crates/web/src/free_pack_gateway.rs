//! Original DROP-03/DROP-06 bridge: private, C-reauthorized Range preflight.
//! This function is intentionally not attached to any HTTP route or S349
//! grant. There are no client-selected ranges or full PACK presigned URLs.
use sha2::{Digest, Sha256};
use tor_books::repositories::free_pack_attach::PgFreePackCatalog;
use tor_reading::repositories::free_s3_ref::{FreeS3Ref, PgFreeS3Ref};
use tor_storage::{
    domain::provider::StorageError,
    services::{
        free_chapter::FreeChapterObject,
        free_pack_read::{read_reauthorized_pack, PackFrameStore},
        free_placement::{
            plan_after_acl, FreePhysicalPlacement, FreeReadPlan, PackMetadata,
            ValidatedPackReference,
        },
    },
};

#[derive(Clone, Copy)]
pub enum FreePackReadMode {
    Published,
    Saved { version_id: i64 },
}
/// C performs the same live checks as the current S349 SINGLE grant, both
/// before and after S3 Range. B returns only a succeeded, fully verified
/// candidate whose edition hash, size and bucket exactly match C's result.
/// G downloads and decompresses only one verified frame; neither a neighbor
/// nor a whole-pack capability is exposed to the caller.
///
/// Only internal callers may invoke this until the original plan's DROP-06
/// HTTP gateway and separate feature-gated CAS cutover have been accepted.
pub async fn read_authorized_candidate<Store: PackFrameStore>(
    c: &PgFreeS3Ref,
    b: &PgFreePackCatalog,
    s3: &Store,
    chapter: i64,
    session_digest: Option<&[u8; 32]>,
    mode: FreePackReadMode,
) -> Result<Vec<u8>, StorageError> {
    if chapter <= 0 || matches!(mode, FreePackReadMode::Saved { version_id } if version_id <= 0) {
        return Err(StorageError::Invalid);
    }
    let resolve = || async move {
        let result = match mode {
            FreePackReadMode::Published => c.for_session(chapter, session_digest).await,
            FreePackReadMode::Saved { version_id } => {
                c.for_saved_session(chapter, version_id, session_digest)
                    .await
            }
        };
        let authorized: FreeS3Ref = match result.map_err(|_| StorageError::Unavailable)? {
            Some(record) => record,
            None => return Ok(None),
        };
        let candidate = b
            .verified_free_range_candidate(
                authorized.chapter_id,
                authorized.version_id,
                authorized.sha256,
                authorized.size_bytes,
                &authorized.bucket,
            )
            .await
            .map_err(|_| StorageError::Unavailable)?;
        let Some(candidate) = candidate else {
            return Ok(None);
        };
        let edition = FreeChapterObject::from_metadata(
            authorized.chapter_id,
            authorized.version_id,
            authorized.sha256,
            authorized.size_bytes,
        )?;
        let metadata = PackMetadata {
            bucket: candidate.bucket,
            key: candidate.key,
            pack_bytes: candidate.pack_bytes,
            seek_table_start: candidate.seek_table_start,
            frame_offset: candidate.frame_offset,
            frame_bytes: candidate.frame_bytes,
            pack_sha256: candidate.pack_sha256,
            manifest_sha256: candidate.manifest_sha256,
        };
        let reference = ValidatedPackReference::new(edition.clone(), metadata)?;
        let plan: FreeReadPlan = plan_after_acl(&edition, FreePhysicalPlacement::Pack(reference))?;
        Ok(Some(plan))
    };
    read_reauthorized_pack(resolve, s3).await
}

/// DROP-06a: exact ACTIVE free-PACK source, not an R4a candidate. The C
/// authorization has its own active-PACK SQL path because PACK-direct books
/// have no SINGLE receipt. It preserves the S349 public/saved/age/hold ACL.
///
/// On each request: C authorizes -> B resolves immutable active/proven frame
/// -> G performs one bounded private Range GET -> C and B reauthorize
/// the *same* physical plan -> C authorizes once more before response metadata
/// leaves the server. No full PACK signature or S3 key reaches the browser.
pub async fn read_authorized_active<Store: PackFrameStore>(
    c: &PgFreeS3Ref,
    b: &PgFreePackCatalog,
    s3: &Store,
    chapter: i64,
    session_digest: Option<&[u8; 32]>,
    mode: FreePackReadMode,
) -> Result<(FreeS3Ref, Vec<u8>), StorageError> {
    if chapter <= 0 || matches!(mode, FreePackReadMode::Saved { version_id } if version_id <= 0) {
        return Err(StorageError::Invalid);
    }
    let authorized = || async move {
        let result = match mode {
            FreePackReadMode::Published => c.for_active_pack_session(chapter, session_digest).await,
            FreePackReadMode::Saved { version_id } => {
                c.for_saved_active_pack_session(chapter, version_id, session_digest)
                    .await
            }
        };
        result.map_err(|_| StorageError::Unavailable)
    };
    let resolve = || async {
        let source = authorized().await?.ok_or(StorageError::Invalid)?;
        let candidate = b
            .active_verified_free_range(
                source.chapter_id,
                source.version_id,
                source.sha256,
                source.size_bytes,
                &source.bucket,
            )
            .await
            .map_err(|_| StorageError::Unavailable)?
            .ok_or(StorageError::Invalid)?;
        let edition = FreeChapterObject::from_metadata(
            source.chapter_id,
            source.version_id,
            source.sha256,
            source.size_bytes,
        )?;
        let reference = ValidatedPackReference::new(
            edition.clone(),
            PackMetadata {
                bucket: candidate.bucket,
                key: candidate.key,
                pack_bytes: candidate.pack_bytes,
                seek_table_start: candidate.seek_table_start,
                frame_offset: candidate.frame_offset,
                frame_bytes: candidate.frame_bytes,
                pack_sha256: candidate.pack_sha256,
                manifest_sha256: candidate.manifest_sha256,
            },
        )?;
        Ok(Some(plan_after_acl(
            &edition,
            FreePhysicalPlacement::Pack(reference),
        )?))
    };
    let bytes = read_reauthorized_pack(resolve, s3).await?;
    let last = authorized().await?.ok_or(StorageError::Invalid)?;
    let digest: [u8; 32] = Sha256::digest(&bytes).into();
    if last.size_bytes != bytes.len() as u64 || last.sha256 != digest {
        return Err(StorageError::Conflict);
    }
    Ok((last, bytes))
}
