//! DROP-R3b: offline verification of a whole immutable Zstd seekable PACK.
//!
//! B must supply the authoritative exact FREE editions, whole-PACK and
//! sidecar SHA256s, object lengths and keys from its trusted publication job.
//! S3 bytes, JSON and embedded seek-table are untrusted until checked here.
//! No HTTP route or SQL publication is connected to this proof primitive.

use serde::Deserialize;
use std::collections::{HashMap, HashSet};

use crate::{
    domain::provider::{valid_private_bucket, StorageError},
    services::{
        crypto::sha256_bytes,
        free_chapter::FreeChapterObject,
        free_pack_read::decode_exact_frame,
        free_placement::{PackMetadata, ValidatedPackReference, MAX_FREE_PACK_BYTES},
    },
};

pub const MAX_PACK_MANIFEST_BYTES: u64 = 1024 * 1024;
pub const MAX_PROOF_MEMBERS: usize = 100_000;
const SKIPPABLE_MAGIC: u32 = 0x184D2A5E;
const SEEKABLE_MAGIC: u32 = 0x8F92EAB1;

/// One *trusted* PostgreSQL receipt candidate, not an S3 ACL capability.
#[derive(Clone, Debug)]
pub struct PackProofTarget {
    pub bucket: String,
    pub pack_key: String,
    pub manifest_key: String,
    pub pack_sha256: [u8; 32],
    pub manifest_sha256: [u8; 32],
    pub pack_bytes: u64,
    pub manifest_bytes: u64,
    pub seek_table_start: u64,
}

impl PackProofTarget {
    pub(crate) fn validate(&self) -> Result<(), StorageError> {
        let Some(stem) = self.pack_key.strip_suffix(".zst") else {
            return Err(StorageError::Invalid);
        };
        if !valid_private_bucket(&self.bucket)
            || !self.pack_key.starts_with("private/free-packs/")
            || !self.pack_key.is_ascii()
            || self.pack_key.len() > 240
            || self.pack_key.contains("//")
            || self.pack_key.split('/').any(|segment| {
                segment.is_empty() || segment == "." || segment == ".."
                    || !segment.bytes().all(|b| b.is_ascii_alphanumeric()
                        || matches!(b, b'-' | b'_' | b'.'))
            })
            || self.manifest_key != format!("{stem}.manifest.json")
            || self.pack_sha256 == [0; 32]
            || self.manifest_sha256 == [0; 32]
            || !(26..=MAX_FREE_PACK_BYTES).contains(&self.pack_bytes)
            || !(1..=MAX_PACK_MANIFEST_BYTES).contains(&self.manifest_bytes)
            || self.seek_table_start == 0
            || self.seek_table_start >= self.pack_bytes
        {
            return Err(StorageError::Invalid);
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SeekManifest {
    format: String,
    manifest_version: u32,
    zstd_seekable_version: String,
    pack_sha256: String,
    pack_encoded_bytes: u64,
    seek_table_offset: u64,
    seek_table_bytes: u64,
    frames: usize,
    entries: Vec<ManifestEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestEntry {
    edition_id: String,
    frame: usize,
    offset: u64,
    encoded_bytes: u64,
    raw_bytes: u64,
    sha256: String,
}

/// A proof of the precise physical bytes passed to verify_full_pack.
/// Future B code must ensure these are bounded, independently fetched
/// from an immutable S3 key before recording a verified PG generation.
#[derive(Debug)]
pub struct VerifiedPack {
    frames: Vec<ValidatedPackReference>,
}

impl VerifiedPack {
    pub fn frames(&self) -> &[ValidatedPackReference] {
        &self.frames
    }
}

fn read_u32(bytes: &[u8]) -> Option<u32> {
    let octets: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    Some(u32::from_le_bytes(octets))
}

fn read_table_entry(table: &[u8], frame: usize) -> Option<(u64, u64)> {
    let start = 8usize.checked_add(frame.checked_mul(8)?)?;
    let compressed = u64::from(read_u32(table.get(start..start + 4)?)?);
    let raw = u64::from(read_u32(table.get(start + 4..start + 8)?)?);
    Some((compressed, raw))
}

/// Verify the full binary pack, exact manifest, standard Zstd seekable table
/// and every decoded frame against a complete authorized B edition inventory.
///
/// The sidecar's own pack SHA is not authority. It must match the externally
/// supplied B SHA and full independently fetched object bytes.
pub fn verify_full_pack(
    target: &PackProofTarget,
    expected_editions: &[FreeChapterObject],
    pack: &[u8],
    sidecar: &[u8],
) -> Result<VerifiedPack, StorageError> {
    target.validate()?;
    if expected_editions.is_empty()
        || expected_editions.len() > MAX_PROOF_MEMBERS
        || pack.len() as u64 != target.pack_bytes
        || sidecar.len() as u64 != target.manifest_bytes
        || sha256_bytes(pack) != target.pack_sha256
        || sha256_bytes(sidecar) != target.manifest_sha256
    {
        return Err(StorageError::Corrupt);
    }
    // Typed deserialization rejects duplicate fields and unknown keys.
    let manifest: SeekManifest =
        serde_json::from_slice(sidecar).map_err(|_| StorageError::Corrupt)?;
    let count = expected_editions.len();
    let table_bytes = count
        .checked_mul(8)
        .and_then(|n| n.checked_add(17))
        .ok_or(StorageError::Corrupt)?;
    if manifest.format != "tor-store-public-seekable-zstd"
        || manifest.manifest_version != 1
        || manifest.zstd_seekable_version != "0.1.0"
        || manifest.pack_sha256 != hex::encode(target.pack_sha256)
        || manifest.pack_encoded_bytes != target.pack_bytes
        || manifest.seek_table_offset != target.seek_table_start
        || manifest.seek_table_bytes != table_bytes as u64
        || manifest.frames != count
        || manifest.entries.len() != count
        || target
            .seek_table_start
            .checked_add(table_bytes as u64)
            != Some(target.pack_bytes)
    {
        return Err(StorageError::Corrupt);
    }
    let start = usize::try_from(target.seek_table_start).map_err(|_| StorageError::Corrupt)?;
    let table = pack.get(start..).ok_or(StorageError::Corrupt)?;
    if table.len() != table_bytes
        || read_u32(table.get(..4).ok_or(StorageError::Corrupt)?) != Some(SKIPPABLE_MAGIC)
        || read_u32(table.get(4..8).ok_or(StorageError::Corrupt)?)
            != Some((table_bytes - 8) as u32)
        || read_u32(table.get(table_bytes - 9..table_bytes - 5).ok_or(StorageError::Corrupt)?)
            != Some(count as u32)
        || table[table_bytes - 5] != 0
        || read_u32(table.get(table_bytes - 4..).ok_or(StorageError::Corrupt)?) != Some(SEEKABLE_MAGIC)
    {
        return Err(StorageError::Corrupt);
    }

    let mut authoritative = HashMap::with_capacity(count);
    for edition in expected_editions {
        let identity = format!("{}:{}", edition.chapter_id(), edition.version_id());
        if authoritative.insert(identity, edition).is_some() {
            return Err(StorageError::Invalid);
        }
    }
    let mut seen = HashSet::with_capacity(count);
    let mut frames = Vec::with_capacity(count);
    let mut offset = 0u64;
    for (index, entry) in manifest.entries.iter().enumerate() {
        let authoritative_edition = authoritative
            .get(&entry.edition_id)
            .ok_or(StorageError::Corrupt)?;
        if !seen.insert(entry.edition_id.as_str())
            || entry.frame != index
            || entry.offset != offset
            || entry.sha256 != hex::encode(authoritative_edition.sha256())
            || entry.raw_bytes != authoritative_edition.size_bytes()
        {
            return Err(StorageError::Corrupt);
        }
        let (compressed, decompressed) = read_table_entry(table, index)
            .ok_or(StorageError::Corrupt)?;
        if compressed == 0
            || compressed != entry.encoded_bytes
            || decompressed != entry.raw_bytes
        {
            return Err(StorageError::Corrupt);
        }
        let end = offset.checked_add(compressed).ok_or(StorageError::Corrupt)?;
        if end > target.seek_table_start {
            return Err(StorageError::Corrupt);
        }
        let frame = ValidatedPackReference::new(
            (*authoritative_edition).clone(),
            PackMetadata {
                bucket: target.bucket.clone(),
                key: target.pack_key.clone(),
                pack_bytes: target.pack_bytes,
                seek_table_start: target.seek_table_start,
                frame_offset: offset,
                frame_bytes: compressed,
                pack_sha256: target.pack_sha256,
                manifest_sha256: target.manifest_sha256,
            },
        )?;
        let start = usize::try_from(offset).map_err(|_| StorageError::Corrupt)?;
        let end_usize = usize::try_from(end).map_err(|_| StorageError::Corrupt)?;
        decode_exact_frame(&frame, pack.get(start..end_usize).ok_or(StorageError::Corrupt)?)?;
        frames.push(frame);
        offset = end;
    }
    if offset != target.seek_table_start || seen.len() != count {
        return Err(StorageError::Corrupt);
    }
    Ok(VerifiedPack { frames })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn fixture() -> (PackProofTarget, Vec<FreeChapterObject>, Vec<u8>, Vec<u8>) {
        let chapters = [
            (12, 22, "# Один\n\nПервая глава."),
            (13, 23, "# Два\n\nДругая глава 中文😀."),
        ];
        let mut pack = Vec::new();
        let mut entries = Vec::new();
        let mut table = Vec::new();
        let mut editions = Vec::new();
        for (index, (chapter, version, raw)) in chapters.iter().enumerate() {
            let edition = FreeChapterObject::for_markdown(*chapter, *version, raw).unwrap();
            let encoded = zstd::bulk::compress(raw.as_bytes(), 3).unwrap();
            entries.push(json!({
                "edition_id": format!("{chapter}:{version}"),
                "frame": index,
                "offset": pack.len(),
                "encoded_bytes": encoded.len(),
                "raw_bytes": raw.len(),
                "sha256": hex::encode(edition.sha256()),
            }));
            table.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
            table.extend_from_slice(&(raw.len() as u32).to_le_bytes());
            editions.push(edition);
            pack.extend_from_slice(&encoded);
        }
        let seek = pack.len();
        pack.extend_from_slice(&SKIPPABLE_MAGIC.to_le_bytes());
        pack.extend_from_slice(&(table.len() as u32 + 9).to_le_bytes());
        pack.extend_from_slice(&table);
        pack.extend_from_slice(&(editions.len() as u32).to_le_bytes());
        pack.push(0);
        pack.extend_from_slice(&SEEKABLE_MAGIC.to_le_bytes());
        let sidecar = serde_json::to_vec(&json!({
            "format": "tor-store-public-seekable-zstd",
            "manifest_version": 1,
            "zstd_seekable_version": "0.1.0",
            "pack_sha256": hex::encode(sha256_bytes(&pack)),
            "pack_encoded_bytes": pack.len(),
            "seek_table_offset": seek,
            "seek_table_bytes": pack.len() - seek,
            "frames": editions.len(),
            "entries": entries,
        }))
        .unwrap();
        let target = PackProofTarget {
            bucket: "tor-private-free-ci".to_owned(),
            pack_key: "private/free-packs/ci/proof-r3b.zst".to_owned(),
            manifest_key: "private/free-packs/ci/proof-r3b.manifest.json".to_owned(),
            pack_sha256: sha256_bytes(&pack),
            manifest_sha256: sha256_bytes(&sidecar),
            pack_bytes: pack.len() as u64,
            manifest_bytes: sidecar.len() as u64,
            seek_table_start: seek as u64,
        };
        (target, editions, pack, sidecar)
    }

    #[test]
    fn real_standard_seekable_table_and_full_sidecar_match_two_b_editions() {
        let (target, editions, pack, sidecar) = fixture();
        let valid = verify_full_pack(&target, &editions, &pack, &sidecar).unwrap();
        assert_eq!(valid.frames().len(), 2);
        assert_eq!(valid.frames()[0].edition(), &editions[0]);
        assert_eq!(valid.frames()[1].edition(), &editions[1]);
        assert_eq!(valid.frames()[1].gateway_range().starts_with("bytes="), true);
    }

    #[test]
    fn one_changed_byte_in_pack_manifest_or_seek_table_is_rejected() {
        let (target, editions, pack, sidecar) = fixture();
        let mut corrupt_pack = pack.clone();
        corrupt_pack[1] ^= 1;
        assert_eq!(
            verify_full_pack(&target, &editions, &corrupt_pack, &sidecar).err(),
            Some(StorageError::Corrupt)
        );
        let mut corrupt_table = pack.clone();
        *corrupt_table.last_mut().unwrap() ^= 1;
        assert_eq!(
            verify_full_pack(&target, &editions, &corrupt_table, &sidecar).err(),
            Some(StorageError::Corrupt)
        );
        let mut manifest = sidecar.clone();
        *manifest.last_mut().unwrap() ^= 1;
        assert_eq!(
            verify_full_pack(&target, &editions, &pack, &manifest).err(),
            Some(StorageError::Corrupt)
        );
    }

    #[test]
    fn no_missing_extra_or_foreign_edition_can_enter_verified_pack() {
        let (target, editions, pack, sidecar) = fixture();
        assert_eq!(
            verify_full_pack(&target, &editions[..1], &pack, &sidecar).err(),
            Some(StorageError::Corrupt)
        );
        let extra = FreeChapterObject::for_markdown(99, 109, "extra text").unwrap();
        let mut wrong = editions.clone();
        wrong[1] = extra;
        assert_eq!(
            verify_full_pack(&target, &wrong, &pack, &sidecar).err(),
            Some(StorageError::Corrupt)
        );
        let mut wrong_digest = editions.clone();
        wrong_digest[1] = FreeChapterObject::for_markdown(13, 23, "other text").unwrap();
        assert_eq!(
            verify_full_pack(&target, &wrong_digest, &pack, &sidecar).err(),
            Some(StorageError::Corrupt)
        );
    }

    #[test]
    fn misleading_or_oversized_receipt_cannot_be_marked_verified() {
        let (target, editions, pack, sidecar) = fixture();
        let mut forged = target.clone();
        forged.seek_table_start -= 1;
        assert_eq!(
            verify_full_pack(&forged, &editions, &pack, &sidecar).err(),
            Some(StorageError::Corrupt)
        );
        let mut forged = target.clone();
        forged.pack_bytes = MAX_FREE_PACK_BYTES + 1;
        assert_eq!(
            verify_full_pack(&forged, &editions, &pack, &sidecar).err(),
            Some(StorageError::Invalid)
        );
        let mut forged = target.clone();
        forged.manifest_key = "public/leaked.json".into();
        assert_eq!(
            verify_full_pack(&forged, &editions, &pack, &sidecar).err(),
            Some(StorageError::Invalid)
        );
        let mut forged = target.clone();
        forged.manifest_bytes = MAX_PACK_MANIFEST_BYTES + 1;
        assert_eq!(
            verify_full_pack(&forged, &editions, &pack, &sidecar).err(),
            Some(StorageError::Invalid)
        );
    }

    #[test]
    fn altered_sidecar_semantics_are_rejected_even_when_its_digest_is_updated() {
        let (target, editions, pack, sidecar) = fixture();
        let mut manifest: serde_json::Value = serde_json::from_slice(&sidecar).unwrap();
        manifest["entries"][1]["offset"] = serde_json::Value::from(0);
        let forged_sidecar = serde_json::to_vec(&manifest).unwrap();
        let mut forged_target = target.clone();
        forged_target.manifest_sha256 = sha256_bytes(&forged_sidecar);
        forged_target.manifest_bytes = forged_sidecar.len() as u64;
        assert_eq!(
            verify_full_pack(&forged_target, &editions, &pack, &forged_sidecar).err(),
            Some(StorageError::Corrupt)
        );
    }
}
