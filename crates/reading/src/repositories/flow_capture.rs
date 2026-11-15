//! Optional C-owned FLOW facts. Business writes remain authoritative.
//! Counters describe this process only; they cannot establish historical coverage.
use crate::services::{progress::ReadingPlace, sync::SyncError};
use sqlx::{PgPool, Postgres, Transaction};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

static ACTION_FAILURES: AtomicU64 = AtomicU64::new(0);
static READ_FAILURES: AtomicU64 = AtomicU64::new(0);
static ACTIVITY_FAILURES: AtomicU64 = AtomicU64::new(0);
static MAINTENANCE_FAILURES: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FlowCaptureHealth {
    pub action_failures: u64,
    pub read_failures: u64,
    pub activity_failures: u64,
    pub maintenance_failures: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowCaptureError {
    Unavailable,
}

#[derive(Clone)]
pub struct PgFlowCapture {
    pool: PgPool,
}

impl PgFlowCapture {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub fn health(&self) -> FlowCaptureHealth {
        FlowCaptureHealth {
            action_failures: ACTION_FAILURES.load(Ordering::Relaxed),
            read_failures: READ_FAILURES.load(Ordering::Relaxed),
            activity_failures: ACTIVITY_FAILURES.load(Ordering::Relaxed),
            maintenance_failures: MAINTENANCE_FAILURES.load(Ordering::Relaxed),
        }
    }

    /// Run at every PostgreSQL startup and periodically, including collection-off.
    /// Each small batch commits independently; an unfinished backlog is an error,
    /// not a successful retention/coverage claim. The next tick resumes cleanup.
    pub async fn maintain(&self) -> Result<(), FlowCaptureError> {
        let started = Instant::now();
        for _ in 0..16 {
            match self.maintain_batch().await {
                Ok(true) => return Ok(()),
                Ok(false) if started.elapsed() < Duration::from_millis(500) => {}
                _ => break,
            }
        }
        MAINTENANCE_FAILURES.fetch_add(1, Ordering::Relaxed);
        Err(FlowCaptureError::Unavailable)
    }

    async fn maintain_batch(&self) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("SET LOCAL statement_timeout='750ms'")
            .execute(&mut *tx)
            .await?;
        sqlx::query("SET LOCAL lock_timeout='25ms'")
            .execute(&mut *tx)
            .await?;
        let actions = sqlx::query(
            "DELETE FROM reader_flow_book_actions WHERE event_id IN (
               SELECT event_id FROM reader_flow_book_actions
               WHERE occurred_at<clock_timestamp()-interval '30 days'
               ORDER BY occurred_at,event_id LIMIT 1000)",
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        let days = sqlx::query(
            "DELETE FROM reader_flow_activity_days
             WHERE (subject_id,flow_epoch,book_id,activity_day) IN (
               SELECT subject_id,flow_epoch,book_id,activity_day
               FROM reader_flow_activity_days
               WHERE first_seen_at<clock_timestamp()-interval '30 days'
               ORDER BY first_seen_at,subject_id,flow_epoch,book_id,activity_day LIMIT 1000)",
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        let reads = sqlx::query(
            "DELETE FROM reader_flow_read_receipts WHERE event_id IN (
               SELECT event_id FROM reader_flow_read_receipts
               WHERE occurred_at<clock_timestamp()-interval '30 days'
               ORDER BY occurred_at,event_id LIMIT 1000)",
        )
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(actions < 1000 && days < 1000 && reads < 1000)
    }
}

/// A local gate is set afresh for every command, including disabled commands.
/// Neither pooled connections nor a prior command can accidentally opt in.
pub(super) async fn prepare_action(
    tx: &mut Transaction<'_, Postgres>,
    enabled: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "SELECT set_config('tor.flow_capture_enabled',$1,true),
                set_config('tor.flow_capture_failed','0',true)",
    )
    .bind(if enabled { "1" } else { "0" })
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn action_failed(tx: &mut Transaction<'_, Postgres>) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar("SELECT COALESCE(current_setting('tor.flow_capture_failed',true),'0')='1'")
        .fetch_one(&mut **tx)
        .await
}

pub(super) fn action_committed(failed: bool) {
    if failed {
        ACTION_FAILURES.fetch_add(1, Ordering::Relaxed);
    }
}

/// Canonical C events are written even when FLOW is disabled. Only this
/// separate command gate permits I's trigger to record consent-scoped proof.
pub(super) async fn prepare_read(
    tx: &mut Transaction<'_, Postgres>,
    enabled: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "SELECT set_config('tor.flow_read_capture_enabled',$1,true),
                set_config('tor.flow_read_capture_failed','0',true)",
    )
    .bind(if enabled { "1" } else { "0" })
    .execute(&mut **tx)
    .await?;
    Ok(())
}

pub(super) async fn read_failed(tx: &mut Transaction<'_, Postgres>) -> Result<bool, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT COALESCE(current_setting('tor.flow_read_capture_failed',true),'0')='1'",
    )
    .fetch_one(&mut **tx)
    .await
}

pub(super) fn read_committed(failed: bool) {
    if failed {
        READ_FAILURES.fetch_add(1, Ordering::Relaxed);
    }
}

/// Called only after C accepted a positive validated advance. It deliberately
/// ignores the old global/book return flags and the old completed D30 marker.
pub(super) async fn record_activity(
    tx: &mut Transaction<'_, Postgres>,
    actor: i64,
    book: i64,
    place: ReadingPlace,
    enabled: bool,
) -> Result<(), SyncError> {
    if !enabled || (place.block_index == 1 && place.char_offset == 0) {
        return Ok(());
    }
    sqlx::query("SAVEPOINT reader_flow_activity_capture")
        .execute(&mut **tx)
        .await
        .map_err(|_| SyncError::Unavailable)?;
    if let Err(error) = capture_activity(tx, actor, book).await {
        ACTIVITY_FAILURES.fetch_add(1, Ordering::Relaxed);
        sqlx::query("ROLLBACK TO SAVEPOINT reader_flow_activity_capture")
            .execute(&mut **tx)
            .await
            .map_err(|_| SyncError::Unavailable)?;
        // Explicit cancellation is not an optional analytics failure. Preserve
        // its rollback semantics rather than turning a cancelled command green.
        if error
            .as_database_error()
            .and_then(|e| e.code())
            .is_some_and(|code| code == "57014")
        {
            return Err(SyncError::Unavailable);
        }
    }
    sqlx::query("RELEASE SAVEPOINT reader_flow_activity_capture")
        .execute(&mut **tx)
        .await
        .map_err(|_| SyncError::Unavailable)?;
    Ok(())
}

async fn capture_activity(
    tx: &mut Transaction<'_, Postgres>,
    actor: i64,
    book: i64,
) -> Result<(), sqlx::Error> {
    let previous_timeout: String = sqlx::query_scalar("SHOW lock_timeout")
        .fetch_one(&mut **tx)
        .await?;
    sqlx::query("SET LOCAL lock_timeout='25ms'")
        .execute(&mut **tx)
        .await?;
    // The approved F projection locks the current scope, serializing opt-out,
    // re-consent and account deletion. C's accepted position uses transaction
    // time: waiting on this optional scope lock must not move that earlier
    // command into an epoch enabled while it waited. No F table is accessed.
    sqlx::query(
        "WITH scope AS MATERIALIZED (
           SELECT subject_id,flow_epoch,flow_enabled_at FROM reader_flow_consent
           WHERE user_id=$1 FOR UPDATE
         ), observed AS MATERIALIZED (SELECT transaction_timestamp() AS at)
         INSERT INTO reader_flow_activity_days
           (subject_id,flow_epoch,book_id,activity_day,first_seen_at)
         SELECT scope.subject_id,scope.flow_epoch,$2,(at AT TIME ZONE 'UTC')::date,at
         FROM scope CROSS JOIN observed WHERE at>=scope.flow_enabled_at
         ON CONFLICT(subject_id,flow_epoch,book_id,activity_day) DO NOTHING",
    )
    .bind(actor)
    .bind(book)
    .execute(&mut **tx)
    .await?;
    sqlx::query("SELECT set_config('lock_timeout',$1,true)")
        .bind(previous_timeout)
        .execute(&mut **tx)
        .await?;
    Ok(())
}
