//! DROP-B11: offline candidate evaluation from a fully verified immutable PACK.
use crate::{
    domain::provider::StorageError,
    services::{
        crypto::sha256_bytes,
        free_pack_proof::VerifiedPack,
        free_pack_read::decode_exact_frame,
        free_pack_write::{prepare_free_pack, PreparedFreePack},
    },
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Retention {
    /// Includes active editions, saved V1, backup and pending references.
    Retain,
    /// An external retention policy explicitly allowed retirement.
    Expired,
    Unknown,
}

pub struct MemberRetention {
    pub chapter_id: i64,
    pub version_id: i64,
    pub retention: Retention,
}

pub struct CompactionBudget {
    pub max_read_bytes: u64,
    pub max_write_bytes: u64,
    pub min_saved_bytes: u64,
}

#[derive(Debug, Eq, PartialEq)]
pub enum CompactionSkip {
    UncertainReference,
    NoExpirations,
    NoRetainedMembers,
    ReadBudget,
    WriteBudget,
    InsufficientSavings,
}

pub struct CompactionProposal {
    pub replacement: PreparedFreePack,
    pub source_read_bytes: u64,
    pub replacement_write_bytes: u64,
    /// Potential after separately approved retirement, not real free space.
    pub potential_saved_bytes: u64,
}

/// Projected costs for a store that keeps a copy of both PACK and manifest.
/// All tariffs must use the same currency and be supplied by the operator.
#[derive(Clone, Copy, Debug)]
pub struct CompactionStoreCost {
    pub storage_micro_per_gib_month: u64,
    pub reads_micro_per_thousand: u64,
    pub writes_micro_per_thousand: u64,
    pub reads: u32,
    pub writes: u32,
    pub overlap_months: u16,
    pub months_after_retirement: u16,
}

impl CompactionProposal {
    /// Savings relative to retaining the source. A positive result requires
    /// savings after retirement to cover overlap and request costs across
    /// all copies. Round toward zero in whole micro units.
    pub fn estimated_savings_micro(
        &self,
        stores: &[CompactionStoreCost],
    ) -> Result<i128, StorageError> {
        const GIB: i128 = 1 << 30;
        const REQUEST_UNIT: i128 = 1_000;
        if stores.is_empty() {
            return Err(StorageError::Invalid);
        }

        let mut total = 0i128;
        for store in stores {
            let saved_byte_months =
                i128::from(self.potential_saved_bytes) * i128::from(store.months_after_retirement);
            let overlap_byte_months =
                i128::from(self.replacement_write_bytes) * i128::from(store.overlap_months);
            let storage_savings = (saved_byte_months - overlap_byte_months)
                .checked_mul(i128::from(store.storage_micro_per_gib_month))
                .and_then(|value| value.checked_mul(REQUEST_UNIT))
                .ok_or(StorageError::Invalid)?;
            let requests = i128::from(store.reads) * i128::from(store.reads_micro_per_thousand)
                + i128::from(store.writes) * i128::from(store.writes_micro_per_thousand);
            total = total
                .checked_add(storage_savings)
                .and_then(|value| value.checked_sub(requests * GIB))
                .ok_or(StorageError::Invalid)?;
        }
        Ok(total / (GIB * REQUEST_UNIT))
    }
}

pub enum CompactionAssessment {
    Skip(CompactionSkip),
    Proposed(Box<CompactionProposal>),
}

/// Assess one rewrite without storage or database I/O. Source proof and the
/// complete per-edition retention inventory are required. This does not
/// authorize uploading, changing active generations or deleting the source.
pub fn assess_compaction(
    proof: &VerifiedPack,
    pack: &[u8],
    members: &[MemberRetention],
    replacement_key: &str,
    budget: &CompactionBudget,
) -> Result<CompactionAssessment, StorageError> {
    let target = proof.target();
    let frames = proof.frames();
    if replacement_key == target.pack_key
        || members.len() != frames.len()
        || frames.iter().zip(members).any(|(frame, member)| {
            frame.edition().chapter_id() != member.chapter_id
                || frame.edition().version_id() != member.version_id
        })
    {
        return Err(StorageError::Invalid);
    }
    if pack.len() as u64 != target.pack_bytes || sha256_bytes(pack) != target.pack_sha256 {
        return Err(StorageError::Corrupt);
    }
    if members
        .iter()
        .any(|item| item.retention == Retention::Unknown)
    {
        return Ok(CompactionAssessment::Skip(
            CompactionSkip::UncertainReference,
        ));
    }
    let retained_count = members
        .iter()
        .filter(|item| item.retention == Retention::Retain)
        .count();
    if retained_count == frames.len() {
        return Ok(CompactionAssessment::Skip(CompactionSkip::NoExpirations));
    }
    if retained_count == 0 {
        return Ok(CompactionAssessment::Skip(
            CompactionSkip::NoRetainedMembers,
        ));
    }
    let source_bytes = target.pack_bytes + target.manifest_bytes;
    if source_bytes > budget.max_read_bytes {
        return Ok(CompactionAssessment::Skip(CompactionSkip::ReadBudget));
    }

    let mut retained = Vec::with_capacity(retained_count);
    for (frame, member) in frames.iter().zip(members) {
        if member.retention != Retention::Retain {
            continue;
        }
        let metadata = frame.metadata();
        let offset = metadata.frame_offset as usize;
        let end = (metadata.frame_offset + metadata.frame_bytes) as usize;
        let encoded = pack.get(offset..end).ok_or(StorageError::Corrupt)?;
        retained.push((frame.edition().clone(), decode_exact_frame(frame, encoded)?));
    }
    let replacement = prepare_free_pack(&target.bucket, replacement_key, &retained)?;
    let replacement_bytes = replacement.target().pack_bytes + replacement.target().manifest_bytes;
    if replacement_bytes > budget.max_write_bytes {
        return Ok(CompactionAssessment::Skip(CompactionSkip::WriteBudget));
    }
    let saved = source_bytes.saturating_sub(replacement_bytes);
    if saved == 0 || saved < budget.min_saved_bytes {
        return Ok(CompactionAssessment::Skip(
            CompactionSkip::InsufficientSavings,
        ));
    }
    Ok(CompactionAssessment::Proposed(Box::new(
        CompactionProposal {
            replacement,
            source_read_bytes: source_bytes,
            replacement_write_bytes: replacement_bytes,
            potential_saved_bytes: saved,
        },
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::{free_chapter::FreeChapterObject, free_pack_proof::verify_full_pack};

    #[test]
    fn simulated_rewrite_preserves_pinned_v1_and_refuses_unknown_or_corrupt_evidence() {
        let bucket = "tor-compaction-disposable";
        let replacement_key = "private/free-packs/lab/replacement-pack-b11.zst";
        let data: Vec<_> = [
            "Saved V1 😀",
            "Expired edition",
            "Partially referenced edition",
        ]
        .into_iter()
        .enumerate()
        .map(|(i, text)| {
            (
                FreeChapterObject::for_markdown(10 + i as i64, 20 + i as i64, text).unwrap(),
                text.as_bytes().to_vec(),
            )
        })
        .collect();
        let source =
            prepare_free_pack(bucket, "private/free-packs/lab/source-pack-b11.zst", &data).unwrap();
        let proof = verify_full_pack(
            source.target(),
            source.editions(),
            source.pack_bytes(),
            source.manifest_bytes(),
        )
        .unwrap();
        let mut members = vec![
            MemberRetention {
                chapter_id: 10,
                version_id: 20,
                retention: Retention::Retain,
            },
            MemberRetention {
                chapter_id: 11,
                version_id: 21,
                retention: Retention::Expired,
            },
            MemberRetention {
                chapter_id: 12,
                version_id: 22,
                retention: Retention::Unknown,
            },
        ];
        let budget = CompactionBudget {
            max_read_bytes: 4096,
            max_write_bytes: 4096,
            min_saved_bytes: 1,
        };
        let assess = |bytes: &[u8], members: &[MemberRetention], budget: &CompactionBudget| {
            assess_compaction(&proof, bytes, members, replacement_key, budget)
        };
        assert!(matches!(
            assess(source.pack_bytes(), &members, &budget).unwrap(),
            CompactionAssessment::Skip(CompactionSkip::UncertainReference)
        ));
        members[2].retention = Retention::Retain;
        let CompactionAssessment::Proposed(plan) =
            assess(source.pack_bytes(), &members, &budget).unwrap()
        else {
            panic!("expect a measurable simulation")
        };
        assert_eq!(
            plan.replacement.editions(),
            &[data[0].0.clone(), data[2].0.clone()]
        );
        assert_eq!(plan.replacement.target().pack_key, replacement_key);
        assert_eq!(
            plan.source_read_bytes - plan.replacement_write_bytes,
            plan.potential_saved_bytes
        );
        const GIB_MICRO: u64 = 1 << 30;
        let primary = CompactionStoreCost {
            storage_micro_per_gib_month: GIB_MICRO,
            reads_micro_per_thousand: 1_000,
            writes_micro_per_thousand: 1_000,
            reads: 4,
            writes: 2,
            overlap_months: 1,
            months_after_retirement: 60,
        };
        let primary_savings = i128::from(plan.potential_saved_bytes) * 60
            - i128::from(plan.replacement_write_bytes)
            - 6;
        assert!(primary_savings > 0);
        let archive = CompactionStoreCost {
            months_after_retirement: 0,
            overlap_months: 61,
            ..primary
        };
        assert_eq!(
            plan.estimated_savings_micro(std::slice::from_ref(&primary))
                .unwrap(),
            primary_savings
        );
        assert_eq!(
            plan.estimated_savings_micro(&[primary, archive]).unwrap(),
            primary_savings - i128::from(plan.replacement_write_bytes) * 61 - 6
        );
        let costly_rewrite = CompactionStoreCost {
            writes: u32::MAX,
            ..primary
        };
        assert!(plan.estimated_savings_micro(&[costly_rewrite]).unwrap() < 0);
        assert!(matches!(
            plan.estimated_savings_micro(&[]),
            Err(StorageError::Invalid)
        ));
        let mut mutated = source.pack_bytes().to_vec();
        mutated[0] ^= 1;
        assert!(matches!(
            assess(&mutated, &members, &budget),
            Err(StorageError::Corrupt)
        ));
        let throttled = CompactionBudget {
            max_write_bytes: 1,
            ..budget
        };
        assert!(matches!(
            assess(source.pack_bytes(), &members, &throttled).unwrap(),
            CompactionAssessment::Skip(CompactionSkip::WriteBudget)
        ));
    }
}
