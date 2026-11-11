//! Optional PG-backed RIL-06 pump. Runs outside HTTP and never blocks responses.
//! Enqueue uses a stable UUID across retries; PG claims survive process death.
use crate::{domain::RollupDelta, repositories::PgRilRollups, services::TrafficRecorder};
use super::html_pump::HtmlPump;
use crate::domain::html_health::{HtmlHealthError,HtmlIngressStopped};
use uuid::Uuid;

use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

/// Only this process's RIL pump. No user identity, SQL or batch identifiers.
#[derive(Clone)]
pub struct PgPumpMonitor(Arc<Mutex<PumpVitality>>);

#[derive(Default)]
struct PumpVitality {
    since: Option<Instant>,
    last_started: Option<Instant>,
    last_completed: Option<Instant>,
    last_success: Option<Instant>,
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
    /// Successful local cycle, possibly idle; not proof any pageview reached PG.
    pub age_since_last_success_s: Option<u64>,
    pub running_for_s: Option<u64>,
}

impl Default for PgPumpMonitor {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(PumpVitality {
            since: Some(Instant::now()),
            ..PumpVitality::default()
        })))
    }
}

impl PgPumpMonitor {
    fn begin(&self) {
        let mut vital = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        vital.since.get_or_insert(now);
        vital.last_started = Some(now);
        vital.active = true;
    }

    fn end(&self, failed: bool) {
        let mut vital = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        vital.active = false;
        let now = Instant::now();
        vital.last_completed = Some(now);
        vital.cycles = vital.cycles.saturating_add(1);
        if failed {
            vital.failed_cycles = vital.failed_cycles.saturating_add(1);
            vital.consecutive_failures = vital.consecutive_failures.saturating_add(1);
        } else {
            vital.consecutive_failures = 0;
            vital.last_success = Some(now);
        }
    }

    pub fn snapshot(&self) -> PgPumpSnapshot {
        let vital = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let now = Instant::now();
        let elapsed =
            |at: Option<Instant>| at.map(|time| now.saturating_duration_since(time).as_secs());
        PgPumpSnapshot {
            active: vital.active,
            cycles: vital.cycles,
            failed_cycles: vital.failed_cycles,
            consecutive_failures: vital.consecutive_failures,
            age_since_start_s: elapsed(vital.since),
            age_since_last_completion_s: elapsed(vital.last_completed),
            age_since_last_success_s: elapsed(vital.last_success),
            running_for_s: if vital.active {
                elapsed(vital.last_started)
            } else {
                None
            },
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
    html: HtmlPump,
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
        Self {
            html: HtmlPump::new(repo.html_health(), &recorder),
            repo,
            recorder,
            pending: None,
            ticks: 0,
            monitor,
        }
    }

    /// One bounded progress unit. Failures affect *telemetry only*, not HTTP.
    /// If staging succeeded but PG's response was lost, the same UUID is
    /// retried rather than counting the in-memory delta twice.
    pub async fn run_once(&mut self) {
        self.monitor.begin();
        let failed = match tokio::time::timeout(std::time::Duration::from_secs(6), self.progress()).await {
            Ok(failed) => failed,
            Err(_) => { self.html.failure(&self.recorder); true }
        };
        self.monitor.end(failed);
    }
    async fn progress(&mut self) -> bool {
        let healthy = self.html.prepare(&self.recorder).await.is_ok();
        let mut failed = !healthy;

        if self.pending.is_none() && !self.html.waiting() {
            let source = self.recorder.clone();
            let managed = healthy && self.html.has_point();
            match tokio::task::spawn_blocking(move || if managed {source.drain_for_health()} else {source.drain_for_persistence()}).await {
                Ok(Some(delta)) if !delta.is_empty() => {
                    let id=Uuid::new_v4().to_string();
                    if managed {self.html.drained(Some(id.clone()));}
                    self.pending = Some(Pending {
                        id,
                        delta,
                    });
                }
                Ok(Some(_)) => {if managed {self.html.drained(None);}}
                Ok(None) | Err(_) => failed = true,
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
                let _ = self.repo.retry(&claim).await;
                break;
            }
        }
        self.ticks = self.ticks.wrapping_add(1);
        if self.ticks.is_multiple_of(360) && self.repo.prune_expired().await.is_err() {
            failed = true;
        }
        if self.ticks.is_multiple_of(360) && self.html.prune().await.is_err() {failed=true;}
        if self.html.complete(&self.recorder,failed).await.is_err() {failed=true;}
        failed
    }

    /// Call only after the HTTP listener has stopped accepting requests and all
    /// in-flight middleware has drained. Closing the producer alone is not a stop
    /// witness. Pending durable RIL batches remain recoverable; an abort, panic or
    /// Drop without the HTTP witness leaves an unresolved epoch and unknown tail.
    pub async fn shutdown(mut self, witness: HtmlIngressStopped) -> Result<(), HtmlHealthError> {
        // The witness excludes new HTTP producers. Try one final bounded cycle;
        // a pending or failed batch still leaves its tail unacknowledged.
        self.run_once().await;
        self.recorder.close_flow_health();
        self.html.shutdown(&self.recorder, &witness).await
    }

    /// Serializes terminal shutdown with the owned progress loop. An explicit
    /// witness is consumed only between complete progress units. Dropping the
    /// sender or aborting the task leaves the epoch unresolved.
    pub fn spawn_with_shutdown(
        mut self,
        mut stop: tokio::sync::oneshot::Receiver<HtmlIngressStopped>,
    ) -> tokio::task::JoinHandle<Result<(), HtmlHealthError>> {
        tokio::spawn(async move {
            loop {
                match stop.try_recv() {
                    Ok(witness) => return self.shutdown(witness).await,
                    Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
                    Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                        return Err(HtmlHealthError::Unavailable);
                    }
                }
                self.run_once().await;
                match tokio::time::timeout(std::time::Duration::from_secs(10), &mut stop).await {
                    Ok(Ok(witness)) => return self.shutdown(witness).await,
                    Ok(Err(_)) => return Err(HtmlHealthError::Unavailable),
                    Err(_) => {}
                }
            }
        })
    }

    /// One long-lived task per PG-backed web process, sharing its existing pool.
    pub fn spawn_background(mut self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                self.run_once().await;
                tokio::time::sleep(std::time::Duration::from_secs(10)).await;
            }
        })
    }
}

impl Drop for PgRollupPump {
    fn drop(&mut self) { self.recorder.close_flow_health(); }
}

#[cfg(test)]
mod vitality_tests {
    use super::*;

    #[test]
    fn heartbeat_tracks_cycles_and_failure_streak_without_identity() {
        let monitor = PgPumpMonitor::default();
        let first = monitor.snapshot();
        assert_eq!(first.cycles, 0);
        assert_eq!(first.age_since_last_success_s, None);
        assert!(first.age_since_start_s.is_some());
        monitor.begin();
        assert!(monitor.snapshot().active);
        monitor.end(true);
        assert_eq!(monitor.snapshot().consecutive_failures, 1);
        monitor.begin();
        monitor.end(true);
        monitor.begin();
        monitor.end(true);
        let unhealthy = monitor.snapshot();
        assert_eq!(unhealthy.cycles, 3);
        assert_eq!(unhealthy.failed_cycles, 3);
        assert_eq!(unhealthy.consecutive_failures, 3);
        assert_eq!(unhealthy.age_since_last_success_s, None);
        assert!(unhealthy.age_since_last_completion_s.is_some());
        monitor.begin();
        monitor.end(false);
        let healthy = monitor.snapshot();
        assert_eq!(healthy.cycles, 4);
        assert_eq!(healthy.failed_cycles, 3);
        assert_eq!(healthy.consecutive_failures, 0);
        assert!(healthy.age_since_last_success_s.is_some());
        monitor.begin();
        monitor.end(true);
        assert!(monitor.snapshot().age_since_last_success_s.is_some());
        assert!(!format!("{healthy:?}").contains("batch_id"));
    }

    #[test]
    fn an_active_cycle_is_visible_before_completion() {
        let monitor = PgPumpMonitor::default();
        monitor.begin();
        let view = monitor.snapshot();
        assert!(view.active);
        assert!(view.running_for_s.is_some());
        assert_eq!(view.cycles, 0);
    }
}
