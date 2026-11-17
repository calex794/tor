//! DROP-07b: internal, bounded two-direction FREE storage dry run.
//! This module is not wired into HTTP, startup or background tasks.
//! UNKNOWN is NEVER a deletion candidate; output contains aggregate counts,
//! not private object names, hashes or chapter text.
use aws_sdk_s3::Client;
use tor_books::repositories::{
    free_expected_inventory::{FreeInventoryError, PgFreeExpectedInventory},
    free_observed_reconcile::{
        FreeReferenceEvidence, FreeReferenceState, PgFreeObservedClassifier,
    },
    PgCatalog,
};
use tor_storage::{
    domain::provider::{valid_private_bucket, StorageError},
    repositories::{
        free_expected_inventory_s3::{FreeObjectEvidence, S3FreeExpectedObjectVerifier},
        free_inventory_list_s3::{FreeListNamespace, ListedFreeObject, S3FreeInventoryLister},
    },
};

const UNKNOWN_GRACE_SECONDS: i64 = 24 * 60 * 60;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ObservedCounters {
    pub registered: u64,
    pub protected_pending: u64,
    pub unknown_recent: u64,
    pub unknown_aged: u64,
    pub unknown_clock_uncertain: u64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ExpectedCounters {
    pub verified: u64,
    pub size_mismatch: u64,
    pub hash_mismatch: u64,
    pub unverifiable: u64,
}

pub struct ObservedDryRunPage {
    pub counts: ObservedCounters,
    /// Internal opaque provider token. Never place in a public response.
    pub next_token: Option<String>,
}

pub struct ExpectedDryRunPage {
    pub counts: ExpectedCounters,
    /// Internal B keyset cursor. Never place in a public response.
    pub next_after_key: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FreeDryRunError {
    Invalid,
    Unavailable,
}

impl From<FreeInventoryError> for FreeDryRunError {
    fn from(error: FreeInventoryError) -> Self {
        match error {
            FreeInventoryError::Invalid => Self::Invalid,
            FreeInventoryError::Unavailable => Self::Unavailable,
        }
    }
}

impl From<StorageError> for FreeDryRunError {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::Invalid => Self::Invalid,
            StorageError::Conflict | StorageError::Corrupt | StorageError::Unavailable => {
                Self::Unavailable
            }
        }
    }
}

pub struct FreeInventoryDryRun {
    expected: PgFreeExpectedInventory,
    observed: PgFreeObservedClassifier,
    lister: S3FreeInventoryLister,
    verifier: S3FreeExpectedObjectVerifier,
    bucket: String,
}

impl FreeInventoryDryRun {
    /// Strictly internal operator construction. Merely creating the auditor
    /// has no background I/O and enables no DELETE or publication action.
    pub fn new(
        catalog: &PgCatalog,
        client: Client,
        private_bucket: String,
    ) -> Result<Self, FreeDryRunError> {
        if !valid_private_bucket(&private_bucket) {
            return Err(FreeDryRunError::Invalid);
        }
        Ok(Self {
            expected: PgFreeExpectedInventory::new(catalog.pool().clone()),
            observed: PgFreeObservedClassifier::new(catalog.pool().clone()),
            lister: S3FreeInventoryLister::new(client.clone()),
            verifier: S3FreeExpectedObjectVerifier::new(client),
            bucket: private_bucket,
        })
    }

    /// B->G: each exact PG immutable receipt receives its own independent,
    /// bounded private S3 full GET + SHA check. No S3 404 or timeout is ever
    /// converted into "confirmed missing" or a deletion decision.
    pub async fn expected_page(
        &self,
        after_key: Option<&str>,
        limit: u32,
    ) -> Result<ExpectedDryRunPage, FreeDryRunError> {
        let page = self
            .expected
            .expected_page(&self.bucket, after_key, limit)
            .await?;
        let mut counts = ExpectedCounters::default();
        for entry in page.expected {
            match self
                .verifier
                .verify_exact(
                    &self.bucket,
                    &entry.object_key,
                    entry.sha256,
                    entry.size_bytes,
                )
                .await
            {
                Ok(FreeObjectEvidence::Verified) => counts.verified += 1,
                Ok(FreeObjectEvidence::SizeMismatch) => counts.size_mismatch += 1,
                Ok(FreeObjectEvidence::HashMismatch) => counts.hash_mismatch += 1,
                Err(_) => counts.unverifiable += 1,
            }
        }
        Ok(ExpectedDryRunPage {
            counts,
            next_after_key: page.next_after_key,
        })
    }

    /// G->B: enumerate one bounded private namespace page, then classify
    /// every key against *all* protected canonical/staged R4a receipts.
    /// Aged unknown objects remain UNKNOWN, not an orphan/deletion permit.
    pub async fn observed_page(
        &self,
        namespace: FreeListNamespace,
        continuation: Option<&str>,
        limit: u32,
        audit_now_unix: i64,
    ) -> Result<ObservedDryRunPage, FreeDryRunError> {
        if audit_now_unix <= 0 {
            return Err(FreeDryRunError::Invalid);
        }
        let page = self
            .lister
            .list_page(&self.bucket, namespace, continuation, limit)
            .await?;
        let counts = if page.listed.is_empty() {
            ObservedCounters::default()
        } else {
            let keys: Vec<_> = page
                .listed
                .iter()
                .map(|entry| entry.object_key.clone())
                .collect();
            let references = self.observed.classify_page(&self.bucket, &keys).await?;
            summarize_observed(&page.listed, &references, audit_now_unix)?
        };
        Ok(ObservedDryRunPage {
            counts,
            next_token: page.next_token,
        })
    }
}

/// Pure classification: can be regression-tested without S3 or PostgreSQL.
/// Missing timestamps and clock skew are UNKNOWN, not a synthetic grace pass.
fn summarize_observed(
    listed: &[ListedFreeObject],
    references: &[FreeReferenceEvidence],
    now: i64,
) -> Result<ObservedCounters, FreeDryRunError> {
    if listed.len() != references.len() || now <= 0 {
        return Err(FreeDryRunError::Unavailable);
    }
    let mut counts = ObservedCounters::default();
    for record in references {
        let object = listed
            .iter()
            .find(|item| item.object_key == record.object_key)
            .ok_or(FreeDryRunError::Unavailable)?;
        match record.state {
            FreeReferenceState::Registered => counts.registered += 1,
            FreeReferenceState::ProtectedPending => counts.protected_pending += 1,
            FreeReferenceState::Unknown => match object.modified_at_unix {
                Some(modified) if modified >= 0 && modified <= now => {
                    if now.saturating_sub(modified) >= UNKNOWN_GRACE_SECONDS {
                        counts.unknown_aged += 1;
                    } else {
                        counts.unknown_recent += 1;
                    }
                }
                _ => counts.unknown_clock_uncertain += 1,
            },
        }
    }
    Ok(counts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_timestamps_and_old_objects_stay_undecidable_without_gc_rights() {
        let now = 1_900_000_000;
        let keys = [
            (
                "private/free-chapters/1/1.md",
                FreeReferenceState::Registered,
                None,
            ),
            (
                "private/free-chapters/2/2.md",
                FreeReferenceState::ProtectedPending,
                None,
            ),
            (
                "private/free-chapters/3/3.md",
                FreeReferenceState::Unknown,
                Some(now - 100),
            ),
            (
                "private/free-chapters/4/4.md",
                FreeReferenceState::Unknown,
                Some(now - 100_000),
            ),
            (
                "private/free-chapters/5/5.md",
                FreeReferenceState::Unknown,
                Some(now + 100),
            ),
            (
                "private/free-chapters/6/6.md",
                FreeReferenceState::Unknown,
                None,
            ),
        ];
        let listed: Vec<_> = keys
            .iter()
            .map(|(key, _, modified)| ListedFreeObject {
                object_key: (*key).to_owned(),
                size_bytes: 10,
                modified_at_unix: *modified,
            })
            .collect();
        let references: Vec<_> = keys
            .iter()
            .map(|(key, state, _)| FreeReferenceEvidence {
                object_key: (*key).to_owned(),
                state: *state,
            })
            .collect();
        assert_eq!(
            summarize_observed(&listed, &references, now),
            Ok(ObservedCounters {
                registered: 1,
                protected_pending: 1,
                unknown_recent: 1,
                unknown_aged: 1,
                unknown_clock_uncertain: 2,
            })
        );
        assert_eq!(
            summarize_observed(&listed[..2], &references, now),
            Err(FreeDryRunError::Unavailable)
        );
    }
}
