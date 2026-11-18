//! DROP-07d: manual operator-only, ONE S3->PG audit page per invocation.
//!
//! Does not run from HTTP/startup, does not schedule a background job, does
//! not inspect paid objects, and cannot initiate PUT/DELETE/repair/GC.
use std::time::Duration;

use aws_sdk_s3::Client;
use tor_books::repositories::{
    free_inventory_cycles::{
        InventoryCheckpointError, InventoryCycleLease, InventoryDirection, InventoryNamespace,
        InventoryPageCounts, PgFreeInventoryCheckpoints,
    },
    PgCatalog,
};
use tor_storage::repositories::free_inventory_list_s3::FreeListNamespace;

use super::inventory_dry_run::{
    ExpectedCounters, FreeDryRunError, FreeInventoryDryRun, ObservedCounters,
};

const PAGE_LIMIT: u32 = 64;
// At most four bounded 16-MiB byte probes per expected page.
const EXPECTED_PAGE_LIMIT: u32 = 4;
const LEASE_SECONDS: i32 = 120;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperatorScanError {
    Invalid,
    Unavailable,
    StaleLease,
}

impl From<InventoryCheckpointError> for OperatorScanError {
    fn from(error: InventoryCheckpointError) -> Self {
        match error {
            InventoryCheckpointError::Invalid => Self::Invalid,
            InventoryCheckpointError::Conflict => Self::StaleLease,
            InventoryCheckpointError::Unavailable => Self::Unavailable,
        }
    }
}

impl From<FreeDryRunError> for OperatorScanError {
    fn from(error: FreeDryRunError) -> Self {
        match error {
            FreeDryRunError::Invalid => Self::Invalid,
            FreeDryRunError::Unavailable => Self::Unavailable,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OperatorScanCounters {
    pub inspected: u32,
    pub registered: u32,
    pub protected_pending: u32,
    pub unknown: u32,
    pub verified: u32,
    pub mismatch: u32,
    pub unverifiable: u32,
    pub sealed: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperatorScanStep {
    Busy,
    Advanced {
        cycle_id: i64,
        page_index: i64,
        counters: OperatorScanCounters,
    },
    Completed {
        cycle_id: i64,
        page_index: i64,
        counters: OperatorScanCounters,
    },
}

/// Not registered in main, router or any timer. Its construction requires
/// trusted backend PG and a private configured S3 client from the operator.
pub struct OperatorObservedScanner {
    audit: FreeInventoryDryRun,
    checkpoints: PgFreeInventoryCheckpoints,
    bucket: String,
}

fn page_counts(
    observed: ObservedCounters,
) -> Result<(InventoryPageCounts, OperatorScanCounters), OperatorScanError> {
    let unknown = observed
        .unknown_recent
        .checked_add(observed.unknown_aged)
        .and_then(|sum| sum.checked_add(observed.unknown_clock_uncertain))
        .ok_or(OperatorScanError::Invalid)?;
    let inspected = observed
        .registered
        .checked_add(observed.protected_pending)
        .and_then(|sum| sum.checked_add(unknown))
        .ok_or(OperatorScanError::Invalid)?;
    if inspected > u64::from(PAGE_LIMIT) {
        return Err(OperatorScanError::Invalid);
    }
    let inspected = u32::try_from(inspected).map_err(|_| OperatorScanError::Invalid)?;
    let registered = u32::try_from(observed.registered).map_err(|_| OperatorScanError::Invalid)?;
    let protected_pending =
        u32::try_from(observed.protected_pending).map_err(|_| OperatorScanError::Invalid)?;
    let unknown = u32::try_from(unknown).map_err(|_| OperatorScanError::Invalid)?;
    Ok((
        InventoryPageCounts {
            inspected,
            matched: registered,
            protected: protected_pending,
            unknown,
            mismatch: 0,
            unverifiable: 0,
        },
        OperatorScanCounters {
            inspected,
            registered,
            protected_pending,
            unknown,
            ..Default::default()
        },
    ))
}

fn expected_counts(counts: ExpectedCounters) -> InventoryPageCounts {
    // Exactly one outcome is recorded per receipt in expected_page_in_namespace.
    // There are at most EXPECTED_PAGE_LIMIT receipts in this call.
    let matched = counts.verified as u32;
    let mismatch = (counts.hash_mismatch + counts.size_mismatch) as u32;
    let unverifiable = counts.unverifiable as u32;
    let protected = counts.sealed as u32;
    InventoryPageCounts {
        inspected: matched + mismatch + unverifiable + protected,
        matched,
        mismatch,
        unverifiable,
        protected,
        ..Default::default()
    }
}

impl OperatorObservedScanner {
    pub fn new(
        catalog: &PgCatalog,
        s3_client: Client,
        bucket: String,
    ) -> Result<Self, OperatorScanError> {
        let audit = FreeInventoryDryRun::new(catalog, s3_client, bucket.clone())?;
        Ok(Self {
            audit,
            checkpoints: PgFreeInventoryCheckpoints::new(catalog.pool().clone()),
            bucket,
        })
    }

    /// One bounded S3 page + one bounded B classification + one B CAS. Any
    /// S3 timeout/error leaves the existing lease/cursor intact for recovery.
    /// There is deliberately no loop, retry or automatic scheduler here.
    /// Completion is audit progress only, NEVER a GC authorization.
    pub async fn run_one(
        &self,
        namespace: InventoryNamespace,
        audit_now_unix: i64,
    ) -> Result<OperatorScanStep, OperatorScanError> {
        if audit_now_unix <= 0 {
            return Err(OperatorScanError::Invalid);
        }
        let scope = match namespace {
            InventoryNamespace::Single => FreeListNamespace::Single,
            InventoryNamespace::Pack => FreeListNamespace::Pack,
        };
        let Some(lease) = self
            .checkpoints
            .claim(
                &self.bucket,
                InventoryDirection::ObservedToPg,
                namespace,
                LEASE_SECONDS,
            )
            .await?
        else {
            return Ok(OperatorScanStep::Busy);
        };
        // Half the lease duration: the slowest provider call cannot outlive
        // the claim and silently commit a stale count. No heartbeat needed
        // because this invocation is strictly bounded to one page.
        let page = tokio::time::timeout(
            Duration::from_secs((LEASE_SECONDS / 2) as u64),
            self.audit.observed_page(
                scope,
                lease.continuation.as_deref(),
                PAGE_LIMIT,
                audit_now_unix,
            ),
        )
        .await
        .map_err(|_| OperatorScanError::Unavailable)??;
        let (counts, public_counters) = page_counts(page.counts)?;
        self.commit_page(lease, page.next_token, counts, public_counters)
            .await
    }

    /// Follow one namespace's exact PG receipts and verify actual private S3
    /// bytes. An unverifiable GET contributes to the audit uncertainty count.
    pub async fn run_expected_one(
        &self,
        namespace: InventoryNamespace,
    ) -> Result<OperatorScanStep, OperatorScanError> {
        let Some(lease) = self
            .checkpoints
            .claim(
                &self.bucket,
                InventoryDirection::ExpectedToS3,
                namespace,
                LEASE_SECONDS,
            )
            .await?
        else {
            return Ok(OperatorScanStep::Busy);
        };
        let page = tokio::time::timeout(
            Duration::from_secs((LEASE_SECONDS / 2) as u64),
            self.audit.expected_page_in_namespace(
                namespace,
                lease.continuation.as_deref(),
                EXPECTED_PAGE_LIMIT,
            ),
        )
        .await
        .map_err(|_| OperatorScanError::Unavailable)??;
        let counts = expected_counts(page.counts);
        let public = OperatorScanCounters {
            inspected: counts.inspected,
            verified: counts.matched,
            mismatch: counts.mismatch,
            unverifiable: counts.unverifiable,
            sealed: counts.protected,
            ..Default::default()
        };
        self.commit_page(lease, page.next_after_key, counts, public)
            .await
    }

    async fn commit_page(
        &self,
        mut lease: InventoryCycleLease,
        next: Option<String>,
        counts: InventoryPageCounts,
        counters: OperatorScanCounters,
    ) -> Result<OperatorScanStep, OperatorScanError> {
        let complete = next.is_none();
        if !self
            .checkpoints
            .acknowledge_page(&mut lease, next.as_deref(), counts)
            .await?
        {
            return Err(OperatorScanError::StaleLease);
        }
        Ok(if complete {
            OperatorScanStep::Completed {
                cycle_id: lease.id,
                page_index: lease.page_index,
                counters,
            }
        } else {
            OperatorScanStep::Advanced {
                cycle_id: lease.id,
                page_index: lease.page_index,
                counters,
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observed_counters_are_exact_and_never_claim_orphan_status() {
        let (page, public) = page_counts(ObservedCounters {
            registered: 14,
            protected_pending: 4,
            unknown_recent: 5,
            unknown_aged: 2,
            unknown_clock_uncertain: 1,
        })
        .unwrap();
        assert_eq!(page.inspected, 26);
        assert_eq!(page.matched, 14);
        assert_eq!(page.protected, 4);
        assert_eq!(page.unknown, 8);
        assert_eq!(page.mismatch, 0);
        assert_eq!(page.unverifiable, 0);
        assert_eq!(public.unknown, 8);
        assert!(matches!(
            page_counts(ObservedCounters {
                registered: 65,
                ..Default::default()
            }),
            Err(OperatorScanError::Invalid)
        ));
        assert!(matches!(
            page_counts(ObservedCounters {
                unknown_recent: u64::MAX,
                unknown_aged: 1,
                ..Default::default()
            }),
            Err(OperatorScanError::Invalid)
        ));
    }
}
