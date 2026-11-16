//! DROP-R4b2: bounded, deterministic private Zstd seekable PACK construction.
//! B's immutable FREE edition metadata is authority. This creates bytes only;
//! it does not publish, authorize readers, attach a PG placement, or perform I/O.
use std::collections::HashSet;

use serde_json::json;
use tor_contracts::drop_pack::PackPlacementReceipt;

use crate::{
    domain::provider::StorageError,
    services::{
        crypto::sha256_bytes,
        free_chapter::FreeChapterObject,
        free_pack_proof::{
            verify_full_pack, PackProofTarget, MAX_PACK_MANIFEST_BYTES, MAX_PROOF_DECODED_BYTES,
            MAX_PROOF_MEMBERS,
        },
        free_placement::MAX_FREE_PACK_BYTES,
    },
};

const SEEKABLE_SKIPPABLE_MAGIC: u32 = 0x184D2A5E;
const SEEKABLE_FOOTER_MAGIC: u32 = 0x8F92EAB1;

/// In-memory, preflight-verified, immutable source bytes. This is NOT proof
/// that the S3 provider received them; only full independent GET establishes it.
pub struct PreparedFreePack {
    target: PackProofTarget,
    receipt: PackPlacementReceipt,
    editions: Vec<FreeChapterObject>,
    pack: Vec<u8>,
    manifest: Vec<u8>,
}

impl PreparedFreePack {
    pub fn target(&self) -> &PackProofTarget {
        &self.target
    }

    pub fn receipt(&self) -> &PackPlacementReceipt {
        &self.receipt
    }

    pub fn editions(&self) -> &[FreeChapterObject] {
        &self.editions
    }

    pub fn pack_bytes(&self) -> &[u8] {
        &self.pack
    }

    pub fn manifest_bytes(&self) -> &[u8] {
        &self.manifest
    }
}

/// Inputs must originate from the B-approved edition inventory, with each
/// Markdown body read independently from the original verified SINGLE object.
/// Run this CPU-bound constructor in a bounded blocking worker in production.
pub fn prepare_free_pack(
    bucket: &str,
    pack_key: &str,
    sources: &[(FreeChapterObject, Vec<u8>)],
) -> Result<PreparedFreePack, StorageError> {
    if sources.is_empty()
        || sources.len() > MAX_PROOF_MEMBERS
        || !pack_key.starts_with("private/free-packs/")
        || !pack_key.ends_with(".zst")
    {
        return Err(StorageError::Invalid);
    }

    let mut seen = HashSet::with_capacity(sources.len());
    let mut total_decoded = 0_u64;
    for (edition, raw) in sources {
        total_decoded = total_decoded
            .checked_add(edition.size_bytes())
            .ok_or(StorageError::Invalid)?;
        if total_decoded > MAX_PROOF_DECODED_BYTES
            || raw.len() as u64 != edition.size_bytes()
            || sha256_bytes(raw) != *edition.sha256()
            || std::str::from_utf8(raw).is_err()
            || !seen.insert((edition.chapter_id(), edition.version_id()))
        {
            return Err(StorageError::Invalid);
        }
    }

    let mut pack = Vec::new();
    let mut seek_entries = Vec::with_capacity(sources.len() * 8);
    let mut manifest_entries = Vec::with_capacity(sources.len());
    let mut editions = Vec::with_capacity(sources.len());
    for (index, (edition, raw)) in sources.iter().enumerate() {
        let compressed = zstd::bulk::compress(raw, 3).map_err(|_| StorageError::Unavailable)?;
        let frame_bytes = u32::try_from(compressed.len()).map_err(|_| StorageError::Invalid)?;
        let raw_bytes = u32::try_from(raw.len()).map_err(|_| StorageError::Invalid)?;
        let offset = pack.len() as u64;
        if frame_bytes == 0
            || frame_bytes > 204_096
            || offset
                .checked_add(u64::from(frame_bytes))
                .is_none_or(|next| next > MAX_FREE_PACK_BYTES)
        {
            return Err(StorageError::Invalid);
        }
        seek_entries.extend_from_slice(&frame_bytes.to_le_bytes());
        seek_entries.extend_from_slice(&raw_bytes.to_le_bytes());
        manifest_entries.push(json!({
            "edition_id": format!("{}:{}", edition.chapter_id(), edition.version_id()),
            "frame": index,
            "offset": offset,
            "encoded_bytes": frame_bytes,
            "raw_bytes": raw_bytes,
            "sha256": hex::encode(edition.sha256()),
        }));
        editions.push(edition.clone());
        pack.extend_from_slice(&compressed);
    }

    let seek_table_start = pack.len() as u64;
    let table_payload = seek_entries
        .len()
        .checked_add(9)
        .ok_or(StorageError::Invalid)?;
    let table_payload = u32::try_from(table_payload).map_err(|_| StorageError::Invalid)?;
    pack.extend_from_slice(&SEEKABLE_SKIPPABLE_MAGIC.to_le_bytes());
    pack.extend_from_slice(&table_payload.to_le_bytes());
    pack.extend_from_slice(&seek_entries);
    pack.extend_from_slice(&(sources.len() as u32).to_le_bytes());
    pack.push(0);
    pack.extend_from_slice(&SEEKABLE_FOOTER_MAGIC.to_le_bytes());
    if pack.len() as u64 > MAX_FREE_PACK_BYTES {
        return Err(StorageError::Invalid);
    }

    let pack_sha256 = sha256_bytes(&pack);
    let manifest = serde_json::to_vec(&json!({
        "format": "tor-store-public-seekable-zstd",
        "manifest_version": 1,
        "zstd_seekable_version": "0.1.0",
        "pack_sha256": hex::encode(pack_sha256),
        "pack_encoded_bytes": pack.len(),
        "seek_table_offset": seek_table_start,
        "seek_table_bytes": pack.len() as u64 - seek_table_start,
        "frames": sources.len(),
        "entries": manifest_entries,
    }))
    .map_err(|_| StorageError::Invalid)?;
    if manifest.len() as u64 > MAX_PACK_MANIFEST_BYTES {
        return Err(StorageError::Invalid);
    }
    let manifest_key = pack_key.replace(".zst", ".manifest.json");
    let target = PackProofTarget {
        bucket: bucket.to_owned(),
        pack_key: pack_key.to_owned(),
        manifest_key,
        pack_sha256,
        manifest_sha256: sha256_bytes(&manifest),
        pack_bytes: pack.len() as u64,
        manifest_bytes: manifest.len() as u64,
        seek_table_start,
    };
    // The authoritative R3b verifier is the final format oracle, including
    // seek table framing, all identities, every decoded SHA and exact bounds.
    let receipt = verify_full_pack(&target, &editions, &pack, &manifest)?
        .placement_receipt();
    Ok(PreparedFreePack {
        target,
        receipt,
        editions,
        pack,
        manifest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(chapter: i64, version: i64, text: &str) -> (FreeChapterObject, Vec<u8>) {
        (
            FreeChapterObject::for_markdown(chapter, version, text).unwrap(),
            text.as_bytes().to_vec(),
        )
    }

    #[test]
    fn deterministic_seekable_pack_round_trips_exact_unicode_editions() {
        let sources = vec![
            source(10, 21, "First FREE version 😀\\n\\nText"),
            source(10, 22, "Updated V2, saved V1 still exists"),
            source(11, 23, "第二章\\n\\n内容"),
        ];
        let key = "private/free-packs/ci/drop-r4b2-deterministic.zst";
        let one = prepare_free_pack("tor-private-free-ci", key, &sources).unwrap();
        let two = prepare_free_pack("tor-private-free-ci", key, &sources).unwrap();
        assert_eq!(one.pack_bytes(), two.pack_bytes());
        assert_eq!(one.manifest_bytes(), two.manifest_bytes());
        assert_eq!(one.receipt(), two.receipt());
        assert_eq!(one.receipt().frames.len(), 3);
        assert!(one.receipt().frames[0].frame_offset < one.receipt().frames[1].frame_offset);
        assert!(verify_full_pack(one.target(), one.editions(), one.pack_bytes(),
                                 one.manifest_bytes()).is_ok());
    }

    #[test]
    fn rejects_changed_source_duplicate_identity_and_bad_namespace() {
        let mut wrong = source(12, 25, "Correct FREE bytes");
        wrong.1[0] ^= 1;
        assert_eq!(
            prepare_free_pack("tor-private-free-ci",
                "private/free-packs/ci/drop-r4b2-invalid.zst", &[wrong]).err(),
            Some(StorageError::Invalid)
        );
        let same = source(13, 26, "Repeated V1");
        assert_eq!(
            prepare_free_pack("tor-private-free-ci",
                "private/free-packs/ci/drop-r4b2-duplicate.zst",
                &[same.clone(), same]).err(),
            Some(StorageError::Invalid)
        );
        assert_eq!(
            prepare_free_pack("tor-private-free-ci", "public/leak.zst",
                &[source(14, 27, "Private")]).err(),
            Some(StorageError::Invalid)
        );
    }

    #[test]
    fn rejects_cumulative_decompression_budget_before_compression() {
        let text = "a".repeat(200_000);
        let object = FreeChapterObject::for_markdown(18, 28, &text).unwrap();
        let sources: Vec<_> = (0..672)
            .map(|id| {
                (FreeChapterObject::from_metadata(
                    100 + id,
                    200 + id,
                    *object.sha256(),
                    object.size_bytes(),
                ).unwrap(), text.as_bytes().to_vec())
            })
            .collect();
        assert_eq!(
            prepare_free_pack("tor-private-free-ci",
                "private/free-packs/ci/drop-r4b2-budget.zst", &sources).err(),
            Some(StorageError::Invalid)
        );
    }
}
