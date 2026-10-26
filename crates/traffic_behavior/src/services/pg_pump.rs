//! Optional PG-backed RIL-06 pump. Runs outside HTTP and never blocks responses.
//! Enqueue uses a stable UUID across retries; PG claims survive process death.
use crate::{domain::RollupDelta, repositories::PgRilRollups, services::TrafficRecorder};
use uuid::Uuid;

use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

/// Only this process's RIL pump. No user identity, SQL or batch identifiers.
#[derive(Clone, Default)]
pub struct PgPumpMonitor(Arc<Mutex<PumpVitality>>);

#[derive(Default)]
struct PumpVitality {
    since: Option<Instant>,
    last_started: Option<Instant>,
    last_completed: Option<Instant>,
    active: bool,
    cycles: u64,
    failed_cycles: u64,
    consecutive_failures: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PgPumpSnapshot {
    pub active: bool,
    pub cycles: u64,
    pub failed_cycles: u64,
    pub consecutive_failures: u64,
    pub age_since_start_s: Option<u64>,
    pub age_since_last_completion_s: Option<u64>,
    pub running_for_s: Option<u64>,
}

impl PgPumpMonitor {
    fn begin(&self) {
        let mut vital = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        vital.since.get_or_insert(now);
        vital.last_started = Some(now);
        vital.active = true;
    }

    fn end(&self, failed: bool) {
        let mut vital = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        vital.active = false;
        vital.last_completed = Some(Instant::now());
        vital.cycles = vital.cycles.saturating_add(1);
        if failed {
            vital.failed_cycles = vital.failed_cycles.saturating_add(1);
            vital.consecutive_failures = vital.consecutive_failures.saturating_add(1);
        } else {
            vital.consecutive_failures = 0;
        }
    }

    pub fn snapshot(&self) -> PgPumpSnapshot {
        let vital = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        let elapsed = |at: Option<Instant>| at.map(|time| now.saturating_duration_since(time).as_secs());
        PgPumpSnapshot {
            active: vital.active,
            cycles: vital.cycles,
            failed_cycles: vital.failed_cycles,
            consecutive_failures: vital.consecutive_failures,
            age_since_start_s: elapsed(vital.since),
            age_since_last_completion_s: elapsed(vital.last_completed),
            running_for_s: if vital.active { elapsed(vital.last_started) } else { None },
        }
    }
}

struct Pending {
    id: String,
    delta: RollupDelta,
}

pub struct PgRollupPump {
    repo: PgRilRollups,
    recorder: TrafficRecorder,
    pending: Option<Pending>,
    ticks: u64,
    monitor: PgPumpMonitor,
}

impl PgRollupPump {
    pub fn new(repo: PgRilRollups, recorder: TrafficRecorder) -> Self {
        Self::with_monitor(repo, recorder, PgPumpMonitor::default())
    }

    pub fn with_monitor(
        repo: PgRilRollups,
        recorder: TrafficRecorder,
        monitor: PgPumpMonitor,
    ) -> Self {
        Self { repo, recorder, pending: None, ticks: 0, monitor }
    }

    /// One bounded progress unit. Failures affect *telemetry only*, not HTTP.
    /// If staging succeeded but PG's response was lost, the same UUID is
    /// retried rather than counting the in-memory delta twice.
    pub async fn run_once(&mut self) {
        self.monitor.begin();
        let mut failed = false;

        if self.pending.is_none() {
            let source = self.recorder.clone();
            match tokio::task::spawn_blocking(move || source.drain_for_persistence()).await {
                Ok(Some(delta)) if !delta.is_empty() => {
                    self.pending = Some(Pending {
                        id: Uuid::new_v4().to_string(),
                        delta,
                    });
                }
                Ok(Some(_)) => {} // healthy idle cycle
                Ok(None) | Err(_) => failed = true, // worker refused the barrier
            }
        }

        if let Some(pending) = self.pending.as_ref() {
            match self.repo.enqueue(&pending.id, &pending.delta).await {
                Ok(_) => self.pending = None,
                Err(_) => failed = true, // retain exact batch id for retry
            }
        }

        // A different process may have staged batches but crashed. Recover them
        // independently of whether this process managed to enqueue a new batch.
        for _ in 0..4 {
            let claim = match self.repo.claim_next().await {
                Ok(Some(claim)) => claim,
                Ok(None) => break,
                Err(_) => {
                    failed = true;
                    break;
                }
            };
            if self.repo.materialize(&claim).await.is_err() {
                failed = true;
                if self.repo.retry(&claim).await.is_err() {
                    failed = true;
                }
                break;
            }
        }
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks.is_multiple_of(360) && self.repo.prune_expired().await.is_err() {
            failed = true;
        }
        self.monitor.end(failed);
    }

    /// One long-lived task per PG-backed web process, sharing its existing pool.
    pub fn spawn_background(mut self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                self.run_once().await;
            }
        })
    }
}
