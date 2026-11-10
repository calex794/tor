//! Explicitly opted-in G-09 diagnostic scheduler.
//! Schedules HEAD/LIST ONLY (never ciphertext GET, S3 write/delete, GC).
//! One LIST page and one HEAD page for each scope at most once per 15-minute
//! tick. PG checkpoint age + lease/fencing coordinate multiple TOR replicas.
use std::time::Duration;
use tor_accounts::repositories::PgAccounts;
use tor_storage::{
    repositories::{
        reconciliation::{InventoryError, PgPrivateInventory},
        reconciliation_reverse::PgRegisteredAudit,
        reconciliation_schedule::{PgG09Schedule, ScheduledAuditAction},
        s3::S3PrivateStore,
    },
    services::reconciliation::{PrivateInventoryLister, PrivateScope},
};

const PERIOD: Duration = Duration::from_secs(15 * 60);
// Shorter than the repositories' 180-second lease; cancelled requests retain
// the lease until expiry, and are not mistaken for missing S3 objects.
const CALL_TIMEOUT: Duration = Duration::from_secs(150);

/// This task is never created unless BOTH G-09 operator and scheduler flags
/// are explicitly enabled in the authenticated PostgreSQL + private S3 build.
/// Initial 15-minute delay avoids S3 traffic before HTTP binds.
pub(super) fn start(
    accounts: PgAccounts,
    store: S3PrivateStore,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let pool = accounts.pool().clone();
        let due = PgG09Schedule::new(pool.clone());
        let forward = PgPrivateInventory::new(pool.clone());
        let reverse = PgRegisteredAudit::new(pool);
        loop {
            tokio::time::sleep(PERIOD).await;
            for scope in [PrivateScope::Chapter, PrivateScope::ChapterAsset] {
                for reverse_direction in [false, true] {
                    let result = tokio::time::timeout(
                        CALL_TIMEOUT,
                        tick_one(&due, &forward, &reverse, &store, scope, reverse_direction),
                    ).await;
                    match result {
                        Ok(Ok(())) => (),
                        Ok(Err(error)) => {
                            tracing::warn!(
                                ?error,
                                scope = scope.kind(),
                                direction = if reverse_direction { "pg-to-s3" } else { "s3-to-pg" },
                                "G-09 scheduled diagnostic page failed (no DELETE/GET)"
                            );
                        }
                        Err(_) => {
                            tracing::warn!(
                                scope = scope.kind(),
                                direction = if reverse_direction { "pg-to-s3" } else { "s3-to-pg" },
                                "G-09 scheduled diagnostic page timed out; lease will expire"
                            );
                        }
                    }
                }
            }
        }
    })
}

async fn tick_one(
    schedule: &PgG09Schedule,
    forward: &PgPrivateInventory,
    reverse: &PgRegisteredAudit,
    store: &S3PrivateStore,
    scope: PrivateScope,
    reverse_direction: bool,
) -> Result<(), InventoryError> {
    let bucket = PrivateInventoryLister::private_bucket(store);
    match schedule.action(bucket, scope, reverse_direction).await? {
        ScheduledAuditAction::Wait => return Ok(()),
        ScheduledAuditAction::NewCycle => {
            let changed = if reverse_direction {
                reverse.begin_next_cycle(bucket, scope).await?
            } else {
                forward.begin_next_cycle(bucket, scope).await?
            };
            // Another replica or the admin may have already moved this cycle.
            if !changed {
                return Ok(());
            }
        }
        ScheduledAuditAction::ScanPage => (),
    }
    if reverse_direction {
        let _ = reverse.scan_page(store, scope).await?;
    } else {
        let _ = forward.scan_page(store, scope).await?;
    }
    Ok(())
}
