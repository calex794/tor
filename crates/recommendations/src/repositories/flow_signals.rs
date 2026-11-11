//! Durable aggregate signal cursors and event feed. GET never evaluates or writes.
use crate::services::flow_signals::*;
use sqlx::{PgPool, Postgres, Row, Transaction};
use std::{str::FromStr, time::Duration};

mod read;
mod write;

#[derive(Clone)]
pub struct PgFlowSignals {
    pool: PgPool,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SignalLease {
    pub days: i32,
    pub epoch: i64,
    pub started_ms: i64,
    pub until_ms: i64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalObservation {
    pub rule: Rule,
    pub scope: String,
    pub window: SignalWindow,
    pub evidence: Evidence,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalState {
    pub rule: Rule,
    pub days: i32,
    pub scope: String,
    pub window: SignalWindow,
    pub result: Evaluation,
    pub evidence: Evidence,
    pub episode: Episode,
    pub checked_at_ms: i64,
    pub last_epoch: i64,
}
impl SignalState {
    pub fn stale(&self, now_ms: i64) -> bool {
        now_ms < self.checked_at_ms || now_ms.saturating_sub(self.checked_at_ms) > 300_000
    }
    pub fn effective_result(&self, now_ms: i64) -> Evaluation {
        if self.stale(now_ms) {
            Evaluation::new(Status::Unknown, Reason::SourceUnavailable)
        } else {
            self.result
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalEvent {
    pub id: i64,
    pub kind: EventKind,
    pub state: SignalState,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalReport {
    pub checked_at_ms: i64,
    pub days: i32,
    pub states: Vec<SignalState>,
    pub events: Vec<SignalEvent>,
    pub next: Option<i64>,
}
impl PgFlowSignals {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
impl From<sqlx::Error> for SignalError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}
async fn bounded<T>(
    work: impl std::future::Future<Output = Result<T, SignalError>>,
) -> Result<T, SignalError> {
    tokio::time::timeout(Duration::from_millis(950), work)
        .await
        .map_err(|_| SignalError::Unavailable)?
}
async fn settings(tx: &mut Transaction<'_, Postgres>) -> Result<(), SignalError> {
    sqlx::query("SELECT set_config('statement_timeout','750ms',true),set_config('lock_timeout','75ms',true)")
        .execute(&mut **tx).await?;
    Ok(())
}
fn episode(row: &sqlx::postgres::PgRow) -> Result<Episode, SignalError> {
    let reason: Option<String> = row.try_get("episode_reason")?;
    Ok(Episode {
        sequence: row.try_get("episode_seq")?,
        reason: reason.as_deref().map(Reason::from_str).transpose()?,
        opened_ms: row.try_get("episode_opened_ms")?,
        opening_emitted: row.try_get("episode_emitted")?,
        last_notice_ms: row.try_get("last_notice_ms")?,
    })
}
const COLUMNS: &str = "rule,days,scope,status,reason,last_epoch,checked_at_ms,until_epoch,window_label,enabled,available,current_value,previous_value,current_units,previous_units,expected_units,known_discards,source_failed,limited_units,omitted_rows,current_ack_per_mille,previous_ack_per_mille,current_sample,previous_sample,current_opportunities,previous_opportunities,unknown_entities,episode_seq,episode_reason,episode_opened_ms,episode_emitted,last_notice_ms";
