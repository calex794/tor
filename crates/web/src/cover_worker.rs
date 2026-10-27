//! I integration: G's verified image job -> B's fenced cover slot.
//! The optional runner never trusts client-provided job IDs or public S3 URLs.
use async_trait::async_trait;
use std::{
    sync::{Arc, Mutex, OnceLock},
    time::{Duration, Instant},
};
use tokio::{task::JoinHandle, time::MissedTickBehavior};
use tor_books::repositories::cover_slots::PgCoverSlots;
use tor_ingest::{
    domain::cover::CoverError,
    repositories::cover_jobs::PgCoverJobs,
    services::{
        cover_jobs::{BookCoverAttach, CoverObjectStore},
        cover_worker::{CoverWorker, WorkerResult},
    },
};
use tor_books::repositories::PgCatalog;

#[derive(Clone)]
pub struct PgBookCoverBridge {
    slots: PgCoverSlots,
}
impl PgBookCoverBridge {
    pub fn new(slots: PgCoverSlots) -> Self {
        Self { slots }
    }
}

#[async_trait]
impl BookCoverAttach for PgBookCoverBridge {
    async fn attach_ready(&self, job_id: i64, epoch: i64) -> Result<bool, CoverError> {
        self.slots
            .attach_ready_job(job_id, epoch)
            .await
            .map_err(|_| CoverError::Unavailable)
    }
}

/// A local-process observation, not evidence about other nodes or S3 availability.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CoverRunnerSnapshot {
    pub cycles: u64,
    pub attached: u64,
    pub superseded: u64,
    pub retried_or_quarantined: u64,
    pub errors: u64,
    pub timeouts: u64,
    pub consecutive_errors: u64,
    pub active: bool,
    pub stopped: bool,
    pub since_start_s: Option<u64>,
    pub since_completion_s: Option<u64>,
    pub running_for_s: Option<u64>,
}

struct RunnerState {
    since: Instant,
    started: Option<Instant>,
    completed: Option<Instant>,
    metrics: CoverRunnerSnapshot,
}

#[derive(Clone)]
pub struct CoverRunnerMonitor(Arc<Mutex<RunnerState>>);

impl Default for CoverRunnerMonitor {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(RunnerState {
            since: Instant::now(),
            started: None,
            completed: None,
            metrics: CoverRunnerSnapshot::default(),
        })))
    }
}

impl CoverRunnerMonitor {
    fn begin(&self) {
        let mut value = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        value.started = Some(Instant::now());
        value.metrics.active = true;
    }

    fn finish(&self, result: &Result<Result<WorkerResult, CoverError>, tokio::time::error::Elapsed>) {
        let mut value = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        value.completed = Some(Instant::now());
        let counters = &mut value.metrics;
        counters.active = false;
        counters.cycles = counters.cycles.saturating_add(1);
        match result {
            Ok(Ok(WorkerResult::Idle)) => counters.consecutive_errors = 0,
            Ok(Ok(WorkerResult::Attached)) => {
                counters.attached = counters.attached.saturating_add(1);
                counters.consecutive_errors = 0;
            }
            Ok(Ok(WorkerResult::Superseded)) => {
                counters.superseded = counters.superseded.saturating_add(1);
                counters.consecutive_errors = 0;
            }
            Ok(Ok(WorkerResult::RetryOrQuarantine)) => {
                counters.retried_or_quarantined =
                    counters.retried_or_quarantined.saturating_add(1);
                counters.consecutive_errors = counters.consecutive_errors.saturating_add(1);
            }
            Ok(Err(_)) => {
                counters.errors = counters.errors.saturating_add(1);
                counters.consecutive_errors = counters.consecutive_errors.saturating_add(1);
            }
            Err(_) => {
                counters.timeouts = counters.timeouts.saturating_add(1);
                counters.consecutive_errors = counters.consecutive_errors.saturating_add(1);
                // Stop, do not start more jobs while a cancelled spawn_blocking
                // decode may still be finishing on another thread.
                counters.stopped = true;
            }
        }
    }

    pub fn snapshot(&self) -> CoverRunnerSnapshot {
        let value = self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        let elapsed = |time: Option<Instant>| time.map(|v| now.saturating_duration_since(v).as_secs());
        CoverRunnerSnapshot {
            since_start_s: elapsed(Some(value.since)),
            since_completion_s: elapsed(value.completed),
            running_for_s: if value.metrics.active { elapsed(value.started) } else { None },
            ..value.metrics
        }
    }
}

/// The operator must explicitly opt in. Invalid combinations fail startup.
pub fn worker_opt_in(
    setting: Option<&str>,
    demo: bool,
    postgres_configured: bool,
    private_storage: bool,
) -> Result<bool, &'static str> {
    match setting {
        None | Some("0") => Ok(false),
        Some("1") if !demo && postgres_configured && private_storage => Ok(true),
        Some("1") => Err("Cover worker needs PostgreSQL, private S3 and non-demo runtime"),
        Some(_) => Err("TOR_COVER_WORKER_ENABLED must be 0 or 1"),
    }
}

pub struct CoverRunner {
    jobs: PgCoverJobs,
    storage: Arc<dyn CoverObjectStore>,
    books: PgBookCoverBridge,
    monitor: CoverRunnerMonitor,
}

impl CoverRunner {
    pub fn new(catalog: PgCatalog, storage: Arc<dyn CoverObjectStore>, monitor: CoverRunnerMonitor) -> Self {
        let pool = catalog.pool();
        Self {
            jobs: PgCoverJobs::new(pool.clone()),
            storage,
            books: PgBookCoverBridge::new(PgCoverSlots::new(pool)),
            monitor,
        }
    }

    /// False only on timeout. Never start another job after a timed-out CPU task.
    pub async fn run_one_bounded(&self, deadline: Duration) -> bool {
        let worker = CoverWorker {
            jobs: &self.jobs,
            storage: self.storage.as_ref(),
            books: &self.books,
        };
        self.monitor.begin();
        let result = tokio::time::timeout(deadline, worker.run_one()).await;
        self.monitor.finish(&result);
        result.is_ok()
    }

    /// Exactly one leased job at a time. PG lease_epoch fences other processes
    /// and replay after a restart; the runner never bypasses G's claim protocol.
    pub fn spawn(self, interval: Duration, deadline: Duration) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                if !self.run_one_bounded(deadline).await {
                    // A stopped runner is observable on /ops/overview.
                    break;
                }
            }
        })
    }
}

static COVER_MONITOR: OnceLock<CoverRunnerMonitor> = OnceLock::new();

pub fn cover_runner_snapshot() -> Option<CoverRunnerSnapshot> {
    COVER_MONITOR.get().map(CoverRunnerMonitor::snapshot)
}

/// Call only after PostgreSQL readiness, private S3 setup, and an explicit
/// TOR_COVER_WORKER_ENABLED=1; keep the JoinHandle alive until shutdown.
pub fn start_cover_worker_background(
    catalog: PgCatalog,
    storage: Arc<dyn CoverObjectStore>,
) -> Result<JoinHandle<()>, &'static str> {
    let monitor = CoverRunnerMonitor::default();
    COVER_MONITOR
        .set(monitor.clone())
        .map_err(|_| "Cover worker was already started")?;
    Ok(CoverRunner::new(catalog, storage, monitor)
        .spawn(Duration::from_secs(5), Duration::from_secs(90)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_is_default_off_and_fail_closed() {
        assert!(!worker_opt_in(None, false, true, true).unwrap());
        assert!(!worker_opt_in(Some("0"), false, true, true).unwrap());
        assert!(worker_opt_in(Some("1"), false, true, true).unwrap());
        for combination in [
            (true, true, true),
            (false, false, true),
            (false, true, false),
        ] {
            assert!(worker_opt_in(Some("1"), combination.0, combination.1, combination.2).is_err());
        }
        for value in ["yes", "true", "", "2", "01"] {
            assert!(worker_opt_in(Some(value), false, true, true).is_err());
        }
    }

    #[tokio::test]
    async fn local_heartbeat_reports_real_outcomes_and_stops_after_timeout() {
        let monitor = CoverRunnerMonitor::default();
        assert_eq!(monitor.snapshot().cycles, 0);
        monitor.begin();
        assert!(monitor.snapshot().active);
        monitor.finish(&Ok(Ok(WorkerResult::Attached)));
        assert_eq!(monitor.snapshot().attached, 1);
        monitor.begin();
        monitor.finish(&Ok(Ok(WorkerResult::RetryOrQuarantine)));
        assert_eq!(monitor.snapshot().consecutive_errors, 1);
        monitor.begin();
        monitor.finish(&Ok(Err(CoverError::Unavailable)));
        assert_eq!(monitor.snapshot().consecutive_errors, 2);
        monitor.begin();
        let deadline = tokio::time::timeout(Duration::ZERO, std::future::pending::<()>()).await;
        monitor.finish(&deadline.map(|_| Ok(WorkerResult::Idle)));
        let snapshot = monitor.snapshot();
        assert_eq!(snapshot.cycles, 4);
        assert_eq!(snapshot.timeouts, 1);
        assert!(snapshot.stopped);
        assert!(!snapshot.active);
    }
}
