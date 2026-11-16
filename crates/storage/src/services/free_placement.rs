//! DROP R1: logical edition identity is independent of physical S3 placement.
//!
//! This is a non-activating read-plan contract. C must authorize the exact
//! chapter/version BEFORE calling this planner, and must reauthorize immediately
//! before releasing any capability. G's placement is never an entitlement.
//!
//! A full-object S3 presign is valid only for the existing S349 SINGLE format.
//! A PACK may include other readers' chapters: it must be fetched via an
//! independently authorized server-side Range gateway. This module does not
//! implement that gateway, decompress a frame or change live S349 routes.

use crate::{
    domain::provider::{valid_private_bucket, StorageError},
    services::free_chapter::{FreeChapterObject, MAX_FREE_MARKDOWN_BYTES},
};

/// Initial target cap; later profiling may choose smaller immutable packs.
pub const MAX_FREE_PACK_BYTES: u64 = 16 * 1024 * 1024;

/// Bound one compressed Zstd frame, including a conservative codec overhead.
pub const MAX_PACK_FRAME_BYTES: u64 = MAX_FREE_MARKDOWN_BYTES + 4096;

/// Physical metadata obtained from a trusted, versioned PostgreSQL manifest.
///
/// Its public fields are deliberately NOT proof of a valid pack reference.
/// Only [ValidatedPackReference::new] checks bounds and namespace. In a live
/// implementation B/G must additionally verify this manifest and actual S3
/// bytes before a placement can become active.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackMetadata {
    pub bucket: String,
    pub key: String,
    pub pack_bytes: u64,
    pub seek_table_start: u64,
    pub frame_offset: u64,
    pub frame_bytes: u64,
    pub pack_sha256: [u8; 32],
    pub manifest_sha256: [u8; 32],
}

/// A bounded, immutable Zstd seekable frame for exactly one B edition.
///
/// This is NOT authorization, a decoded source, a signed URL or an integrity
/// receipt. The future G gateway must check the Range response, decompress
/// with a strict output bound and compare SHA256 to edition.sha256().
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedPackReference {
    edition: FreeChapterObject,
    metadata: PackMetadata,
}

impl ValidatedPackReference {
    pub fn new(
        edition: FreeChapterObject,
        metadata: PackMetadata,
    ) -> Result<Self, StorageError> {
        if !valid_private_bucket(&metadata.bucket)
            || !valid_pack_key(&metadata.key)
            || metadata.pack_sha256 == [0; 32]
            || metadata.manifest_sha256 == [0; 32]
            || !(1..=MAX_FREE_PACK_BYTES).contains(&metadata.pack_bytes)
            || metadata.seek_table_start == 0
            || metadata.seek_table_start >= metadata.pack_bytes
            || !(1..=MAX_PACK_FRAME_BYTES).contains(&metadata.frame_bytes)
            || metadata
                .frame_offset
                .checked_add(metadata.frame_bytes)
                .is_none_or(|end| end > metadata.seek_table_start)
        {
            return Err(StorageError::Invalid);
        }
        Ok(Self { edition, metadata })
    }

    pub fn edition(&self) -> &FreeChapterObject {
        &self.edition
    }

    pub fn metadata(&self) -> &PackMetadata {
        &self.metadata
    }

    /// Internal server-to-S3 header only, never a browser presigned-pack URL.
    pub fn gateway_range(&self) -> String {
        let end = self.metadata.frame_offset + self.metadata.frame_bytes - 1;
        format!("bytes={}-{}", self.metadata.frame_offset, end)
    }
}

fn valid_pack_key(key: &str) -> bool {
    let Some(rest) = key.strip_prefix("private/free-packs/") else {
        return false;
    };
    !rest.is_empty()
        && rest.len() <= 240
        && rest.ends_with(".zst")
        && !rest.contains("//")
        && rest
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.'))
}

/// A trusted catalog's chosen location for an edition. Neither variant
/// implies that the user can read this edition; C authorizes separately.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FreePhysicalPlacement {
    Single(FreeChapterObject),
    Pack(ValidatedPackReference),
}

/// A transport plan, not a grant. Only Single has a direct S3 presign path.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FreeReadPlan {
    DirectSingle(FreeChapterObject),
    PrivateRangeGateway(ValidatedPackReference),
}

impl FreeReadPlan {
    /// Future signer should call this and fail closed on a PACK.
    pub fn direct_single_object(&self) -> Result<&FreeChapterObject, StorageError> {
        match self {
            Self::DirectSingle(object) => Ok(object),
            Self::PrivateRangeGateway(_) => Err(StorageError::Invalid),
        }
    }

    /// Used only by the future internal, actor-authorized Range gateway.
    pub fn pack_frame(&self) -> Option<&ValidatedPackReference> {
        match self {
            Self::PrivateRangeGateway(frame) => Some(frame),
            Self::DirectSingle(_) => None,
        }
    }
}

/// C supplies an already-authorized immutable edition; G compares *all* its
/// fields against the chosen placement before selecting any read mechanism.
///
/// This does not replace C's session, book/age, hold, saved-pin or publication
/// rules. A live call site must also repeat C authorization after external I/O.
pub fn plan_after_acl(
    authoritative_edition: &FreeChapterObject,
    placement: FreePhysicalPlacement,
) -> Result<FreeReadPlan, StorageError> {
    match placement {
        FreePhysicalPlacement::Single(object) => {
            if &object != authoritative_edition {
                return Err(StorageError::Corrupt);
            }
            Ok(FreeReadPlan::DirectSingle(object))
        }
        FreePhysicalPlacement::Pack(frame) => {
            if frame.edition() != authoritative_edition {
                return Err(StorageError::Corrupt);
            }
            Ok(FreeReadPlan::PrivateRangeGateway(frame))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edition(chapter: i64, version: i64, text: &str) -> FreeChapterObject {
        FreeChapterObject::for_markdown(chapter, version, text).unwrap()
    }

    fn metadata() -> PackMetadata {
        PackMetadata {
            bucket: "tor-private-reads".to_owned(),
            key: "private/free-packs/2026/immutable-8f23.zst".to_owned(),
            pack_bytes: 16_000,
            seek_table_start: 15_000,
            frame_offset: 128,
            frame_bytes: 900,
            pack_sha256: [17; 32],
            manifest_sha256: [18; 32],
        }
    }

    #[test]
    fn canonical_single_can_be_selected_only_for_the_same_edition() {
        let v1 = edition(12, 21, "Текст V1 😀");
        let plan =
            plan_after_acl(&v1, FreePhysicalPlacement::Single(v1.clone())).unwrap();
        assert_eq!(plan.direct_single_object(), Ok(&v1));
        assert!(plan.pack_frame().is_none());
        assert_eq!(
            plan_after_acl(&v1, FreePhysicalPlacement::Single(edition(12, 22, "Текст V2")))
                .err(),
            Some(StorageError::Corrupt)
        );
        assert_eq!(
            plan_after_acl(&v1, FreePhysicalPlacement::Single(edition(12, 21, "Подмена")))
                .err(),
            Some(StorageError::Corrupt)
        );
    }

    #[test]
    fn pack_can_only_select_the_server_side_range_gateway() {
        let v1 = edition(12, 21, "Текст V1 😀");
        let frame = ValidatedPackReference::new(v1.clone(), metadata()).unwrap();
        let plan = plan_after_acl(&v1, FreePhysicalPlacement::Pack(frame.clone())).unwrap();
        assert_eq!(plan.direct_single_object(), Err(StorageError::Invalid));
        assert_eq!(plan.pack_frame(), Some(&frame));
        assert_eq!(frame.gateway_range(), "bytes=128-1027");
        // The same pack may contain other chapters. A signed full-pack GET
        // would allow arbitrary Range requests, so no such plan is exposed.
    }

    #[test]
    fn pack_must_match_exact_version_and_source_digest() {
        let v1 = edition(12, 21, "V1");
        let v2 = edition(12, 22, "V2");
        let frame = ValidatedPackReference::new(v1.clone(), metadata()).unwrap();
        assert_eq!(
            plan_after_acl(&v2, FreePhysicalPlacement::Pack(frame)).err(),
            Some(StorageError::Corrupt)
        );
        let forged = edition(12, 21, "different text");
        let frame = ValidatedPackReference::new(forged, metadata()).unwrap();
        assert_eq!(
            plan_after_acl(&v1, FreePhysicalPlacement::Pack(frame)).err(),
            Some(StorageError::Corrupt)
        );
    }

    #[test]
    fn pack_denies_public_keys_traversal_bad_buckets_and_missing_hashes() {
        let original = edition(12, 21, "V1");
        let wrong_keys = [
            "public/free-packs/readable.zst",
            "private/free-packs/../escape.zst",
            "private/free-packs//empty.zst",
            "private/free-packs/a/./b.zst",
            "private/free-packs/key?download.zst",
            "private/free-packs/object.pack",
            "private/free-packs/",
        ];
        for key in wrong_keys {
            let mut m = metadata();
            m.key = key.to_owned();
            assert_eq!(
                ValidatedPackReference::new(original.clone(), m).err(),
                Some(StorageError::Invalid),
                "{key}"
            );
        }
        let mut m = metadata();
        m.bucket = "public-data".to_owned();
        assert_eq!(
            ValidatedPackReference::new(original.clone(), m).err(),
            Some(StorageError::Invalid)
        );
        let mut m = metadata();
        m.pack_sha256 = [0; 32];
        assert_eq!(
            ValidatedPackReference::new(original.clone(), m).err(),
            Some(StorageError::Invalid)
        );
        let mut m = metadata();
        m.manifest_sha256 = [0; 32];
        assert_eq!(
            ValidatedPackReference::new(original, m).err(),
            Some(StorageError::Invalid)
        );
    }

    #[test]
    fn pack_denies_missing_out_of_bounds_and_overflow_ranges() {
        let original = edition(12, 21, "V1");
        let mut cases = Vec::new();
        let mut m = metadata();
        m.frame_bytes = 0;
        cases.push(m);
        let mut m = metadata();
        m.frame_bytes = MAX_PACK_FRAME_BYTES + 1;
        cases.push(m);
        let mut m = metadata();
        m.frame_offset = 14_900;
        cases.push(m);
        let mut m = metadata();
        m.frame_offset = u64::MAX - 5;
        cases.push(m);
        let mut m = metadata();
        m.seek_table_start = 16_000;
        cases.push(m);
        let mut m = metadata();
        m.pack_bytes = MAX_FREE_PACK_BYTES + 1;
        cases.push(m);
        let mut m = metadata();
        m.pack_bytes = 0;
        cases.push(m);
        for item in cases {
            assert_eq!(
                ValidatedPackReference::new(original.clone(), item).err(),
                Some(StorageError::Invalid)
            );
        }
    }

    #[test]
    fn valid_frame_may_end_exactly_at_seek_table_boundary() {
        let original = edition(20, 30, "Same pack, different chapter");
        let mut m = metadata();
        m.frame_offset = 14_100;
        m.frame_bytes = 900;
        let validated = ValidatedPackReference::new(original, m).unwrap();
        assert_eq!(validated.gateway_range(), "bytes=14100-14999");
    }
}
