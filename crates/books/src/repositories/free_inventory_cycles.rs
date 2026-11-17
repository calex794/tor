//! DROP-07c: recoverable, lease-fenced *read-only audit* checkpoints.
//!
//! A completed cycle records bounded pagination evidence, NOT a consistent
//! cross-system snapshot, orphan proof, backup proof or GC authorization.
use sqlx::{PgPool, Row};

use super::free_expected_inventory::{valid_bucket, valid_cursor};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InventoryDirection {
    ExpectedToS3,
    ObservedToPg,
}

impl InventoryDirection {
    fn as_db(self) -> &'static str {
        match self {
            Self::ExpectedToS3 => "pg_to_s3",
            Self::ObservedToPg => "s3_to_pg",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InventoryNamespace {
    Single,
    Pack,
}

impl InventoryNamespace {
    fn as_db(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Pack => "pack",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InventoryCycleLease {
    pub id: i64,
    pub epoch: i64,
    pub page_index: i64,
    /// Internal opaque pagination token. No public HTTP and no traces.
    pub continuation: Option<String>,
    pub direction: InventoryDirection,
    pub namespace: InventoryNamespace,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InventoryPageCounts {
    pub inspected: u32,
    pub matched: u32,
    pub protected: u32,
    pub unknown: u32,
    pub mismatch: u32,
    pub unverifiable: u32,
}

impl InventoryPageCounts {
    fn valid(self) -> bool {
        let total = [
            self.matched,
            self.protected,
            self.unknown,
            self.mismatch,
            self.unverifiable,
        ]
        .into_iter()
        .try_fold(0_u32, |a, b| a.checked_add(b));
        self.inspected <= 128 && total == Some(self.inspected)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InventoryCheckpointError {
    Invalid,
    Conflict,
    Unavailable,
}

pub struct PgFreeInventoryCheckpoints {
    pool: PgPool,
}

fn valid_token(
    direction: InventoryDirection,
    namespace: InventoryNamespace,
    continuation: Option<&str>,
) -> bool {
    let Some(token) = continuation else {
        return true;
    };
    if token.is_empty() || token.len() > 1024 || token.chars().any(char::is_control) {
        return false;
    }
    if direction == InventoryDirection::ExpectedToS3 {
        valid_cursor(token)
            && token.starts_with(match namespace {
                InventoryNamespace::Single => "private/free-chapters/",
                InventoryNamespace::Pack => "private/free-packs/",
            })
    } else {
        // S3's continuation token is opaque. It is persisted *only* inside
        // this bucket+namespace+direction cycle, never a public input.
        true
    }
}

impl PgFreeInventoryCheckpoints {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Operator-only lease claim. A single running cycle per bucket/namespace/
    /// direction is protected by a unique partial index. Expired leases can
    /// resume the *persisted* token at a greater epoch. Eight thefts/expiries
    /// exhaust the cycle: a fresh audit is required, not an unbounded retry.
    pub async fn claim(
        &self,
        bucket: &str,
        direction: InventoryDirection,
        namespace: InventoryNamespace,
        lease_seconds: i32,
    ) -> Result<Option<InventoryCycleLease>, InventoryCheckpointError> {
        if !valid_bucket(bucket) || !(15..=900).contains(&lease_seconds) {
            return Err(InventoryCheckpointError::Invalid);
        }
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|_| InventoryCheckpointError::Unavailable)?;
        let row = sqlx::query(
            "SELECT id,lease_epoch,page_index,continuation,
                    (locked_until<=clock_timestamp()) AS expired
             FROM free_storage_inventory_cycles
             WHERE bucket=$1 AND direction=$2 AND namespace=$3
               AND status='running'
             FOR UPDATE SKIP LOCKED",
        )
        .bind(bucket)
        .bind(direction.as_db())
        .bind(namespace.as_db())
        .fetch_optional(&mut *tx)
        .await
        .map_err(|_| InventoryCheckpointError::Unavailable)?;
        let claimed = if let Some(row) = row {
            let id: i64 = row
                .try_get("id")
                .map_err(|_| InventoryCheckpointError::Unavailable)?;
            let epoch: i64 = row
                .try_get("lease_epoch")
                .map_err(|_| InventoryCheckpointError::Unavailable)?;
            let expired: bool = row
                .try_get("expired")
                .map_err(|_| InventoryCheckpointError::Unavailable)?;
            if !expired {
                None
            } else if epoch >= 8 {
                sqlx::query(
                    "UPDATE free_storage_inventory_cycles
                     SET status='failed',locked_until=NULL,
                         completed_at=clock_timestamp(),updated_at=clock_timestamp(),
                         failure_code='lease_budget_exhausted'
                     WHERE id=$1 AND lease_epoch=$2 AND status='running'",
                )
                .bind(id)
                .bind(epoch)
                .execute(&mut *tx)
                .await
                .map_err(|_| InventoryCheckpointError::Unavailable)?;
                None
            } else {
                let updated = sqlx::query(
                    "UPDATE free_storage_inventory_cycles
                     SET lease_epoch=lease_epoch+1,
                         locked_until=clock_timestamp()+($2::int*interval '1 second'),
                         updated_at=clock_timestamp()
                     WHERE id=$1 AND lease_epoch=$3 AND status='running'
                     RETURNING lease_epoch,page_index,continuation",
                )
                .bind(id)
                .bind(lease_seconds)
                .bind(epoch)
                .fetch_one(&mut *tx)
                .await
                .map_err(|_| InventoryCheckpointError::Unavailable)?;
                Some(InventoryCycleLease {
                    id,
                    epoch: updated
                        .try_get("lease_epoch")
                        .map_err(|_| InventoryCheckpointError::Unavailable)?,
                    page_index: updated
                        .try_get("page_index")
                        .map_err(|_| InventoryCheckpointError::Unavailable)?,
                    continuation: updated
                        .try_get("continuation")
                        .map_err(|_| InventoryCheckpointError::Unavailable)?,
                    direction,
                    namespace,
                })
            }
        } else {
            // An active row locked by another worker conflicts with this
            // partial UNIQUE index; DO NOTHING rather than racing it.
            let inserted = sqlx::query(
                "INSERT INTO free_storage_inventory_cycles
                 (bucket,direction,namespace,locked_until)
                 VALUES($1,$2,$3,clock_timestamp()+($4::int*interval '1 second'))
                 ON CONFLICT(bucket,direction,namespace) WHERE status='running'
                 DO NOTHING RETURNING id",
            )
            .bind(bucket)
            .bind(direction.as_db())
            .bind(namespace.as_db())
            .bind(lease_seconds)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|_| InventoryCheckpointError::Unavailable)?;
            if let Some(inserted) = inserted {
                Some(InventoryCycleLease {
                    id: inserted
                        .try_get("id")
                        .map_err(|_| InventoryCheckpointError::Unavailable)?,
                    epoch: 1,
                    page_index: 0,
                    continuation: None,
                    direction,
                    namespace,
                })
            } else {
                None
            }
        };
        tx.commit()
            .await
            .map_err(|_| InventoryCheckpointError::Unavailable)?;
        Ok(claimed)
    }

    pub async fn renew(
        &self,
        lease: &InventoryCycleLease,
        seconds: i32,
    ) -> Result<bool, InventoryCheckpointError> {
        if !(15..=900).contains(&seconds) || lease.id <= 0 || lease.epoch <= 0 {
            return Err(InventoryCheckpointError::Invalid);
        }
        let affected = sqlx::query(
            "UPDATE free_storage_inventory_cycles
             SET locked_until=clock_timestamp()+($3::int*interval '1 second'),
                 updated_at=clock_timestamp()
             WHERE id=$1 AND lease_epoch=$2 AND status='running'
               AND locked_until>clock_timestamp()",
        )
        .bind(lease.id)
        .bind(lease.epoch)
        .bind(seconds)
        .execute(&self.pool)
        .await
        .map_err(|_| InventoryCheckpointError::Unavailable)?;
        Ok(affected.rows_affected() == 1)
    }

    /// Atomically acknowledge one bounded page and its aggregate evidence.
    /// A lost COMMIT ACK cannot increment counters twice: page index + old
    /// cursor + distinct lease epoch fence the exact committed transition.
    /// The caller must independently attest that the G/B page is complete.
    pub async fn acknowledge_page(
        &self,
        lease: &mut InventoryCycleLease,
        next_cursor: Option<&str>,
        counts: InventoryPageCounts,
    ) -> Result<bool, InventoryCheckpointError> {
        if lease.id <= 0
            || lease.epoch <= 0
            || !(0..1_000_000).contains(&lease.page_index)
            || !counts.valid()
            || !valid_token(lease.direction, lease.namespace, next_cursor)
            || !valid_token(
                lease.direction,
                lease.namespace,
                lease.continuation.as_deref(),
            )
            || next_cursor == lease.continuation.as_deref() && next_cursor.is_some()
            || (counts.inspected == 0 && next_cursor.is_some())
        {
            return Err(InventoryCheckpointError::Invalid);
        }
        let affected = sqlx::query(
            "UPDATE free_storage_inventory_cycles SET
                 page_index=page_index+1,continuation=$4,
                 inspected_count=inspected_count+$5,
                 matched_count=matched_count+$6,
                 protected_count=protected_count+$7,
                 unknown_count=unknown_count+$8,
                 mismatch_count=mismatch_count+$9,
                 unverifiable_count=unverifiable_count+$10,
                 status=CASE WHEN $4::text IS NULL THEN 'completed' ELSE 'running' END,
                 locked_until=CASE WHEN $4::text IS NULL THEN NULL ELSE locked_until END,
                 completed_at=CASE WHEN $4::text IS NULL THEN clock_timestamp() ELSE NULL END,
                 updated_at=clock_timestamp()
             WHERE id=$1 AND lease_epoch=$2 AND page_index=$3
               AND continuation IS NOT DISTINCT FROM $11::text
               AND status='running' AND locked_until>clock_timestamp()",
        )
        .bind(lease.id)
        .bind(lease.epoch)
        .bind(lease.page_index)
        .bind(next_cursor)
        .bind(i64::from(counts.inspected))
        .bind(i64::from(counts.matched))
        .bind(i64::from(counts.protected))
        .bind(i64::from(counts.unknown))
        .bind(i64::from(counts.mismatch))
        .bind(i64::from(counts.unverifiable))
        .bind(lease.continuation.as_deref())
        .execute(&self.pool)
        .await
        .map_err(|_| InventoryCheckpointError::Unavailable)?;
        if affected.rows_affected() != 1 {
            return Ok(false);
        }
        // Only advance in-memory state after PostgreSQL confirms its CAS.
        // If the COMMIT acknowledgement is lost, an expired-lease reclaim
        // loads the durable page index and cursor instead of double counting.
        lease.page_index += 1;
        lease.continuation = next_cursor.map(str::to_owned);
        Ok(true)
    }

    /// Do not silently mark an interrupted provider scan complete.
    pub async fn fail(
        &self,
        lease: &InventoryCycleLease,
    ) -> Result<bool, InventoryCheckpointError> {
        if lease.id <= 0 || lease.epoch <= 0 {
            return Err(InventoryCheckpointError::Invalid);
        }
        let affected = sqlx::query(
            "UPDATE free_storage_inventory_cycles
             SET status='failed',locked_until=NULL,
                 completed_at=clock_timestamp(),updated_at=clock_timestamp(),
                 failure_code='operator_abort'
             WHERE id=$1 AND lease_epoch=$2 AND status='running'
               AND locked_until>clock_timestamp()",
        )
        .bind(lease.id)
        .bind(lease.epoch)
        .execute(&self.pool)
        .await
        .map_err(|_| InventoryCheckpointError::Unavailable)?;
        Ok(affected.rows_affected() == 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_public_cursor_or_invalid_page_can_be_checkpointed() {
        assert!(!valid_token(
            InventoryDirection::ExpectedToS3,
            InventoryNamespace::Pack,
            Some("private/free-chapters/1/1.md"),
        ));
        assert!(!valid_token(
            InventoryDirection::ObservedToPg,
            InventoryNamespace::Single,
            Some("bad\nopaque"),
        ));
        assert!(valid_token(
            InventoryDirection::ObservedToPg,
            InventoryNamespace::Pack,
            Some("opaque+provider-token"),
        ));
        assert!(!InventoryPageCounts {
            inspected: 1,
            matched: 1,
            unknown: 1,
            ..Default::default()
        }
        .valid());
        assert!(InventoryPageCounts {
            inspected: 2,
            matched: 1,
            protected: 1,
            ..Default::default()
        }
        .valid());
    }
}
