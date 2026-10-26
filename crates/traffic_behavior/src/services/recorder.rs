//! Never block HTTP on analytics. A single bounded worker drains compact events.
//! Full/disconnected queues count dropped telemetry, not failed user requests.
use crate::domain::{
    BehaviorEvent, BehaviorSketch, PathReport, RollupDelta, SketchSnapshot, VerifiedSubjectKey,
    MAX_SUBJECTS,
};
use crate::services::paths_worker::PathRecorder;
use std::collections::{HashSet, VecDeque};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender, TrySendError},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

const MAX_QUEUE: usize = 4_096;
enum Message {
    Event(BehaviorEvent),
    Snapshot(mpsc::Sender<SketchSnapshot>),
    Drain(mpsc::Sender<RollupDelta>),
    Forget(VerifiedSubjectKey, mpsc::Sender<bool>),
}

/// Cloneable, request-safe telemetry sender. No raw path or identity fields.
#[derive(Clone)]
pub struct TrafficRecorder {
    sender: SyncSender<Message>,
    dropped: Arc<AtomicU64>,
    paths: PathRecorder,
    started: Instant,
}

impl TrafficRecorder {
    pub fn start(max_subjects: usize, queue_capacity: usize) -> Self {
        let (sender, receiver) = mpsc::sync_channel(queue_capacity.clamp(1, MAX_QUEUE));
        let dropped = Arc::new(AtomicU64::new(0));
        let paths = PathRecorder::start(max_subjects, queue_capacity);
        let path_sink = paths.clone();
        // Thread creation failure only disables best-effort telemetry.
        let _ = thread::Builder::new()
            .name("tor-ril-traffic".to_owned())
            .spawn(move || {
                let mut sketch = BehaviorSketch::new(max_subjects);
                let mut rollup = RollupDelta::default();
                let mut revoked = HashSet::<VerifiedSubjectKey>::new();
                let mut expirations = VecDeque::<(Instant, VerifiedSubjectKey)>::new();
                let mut fail_closed = false;
                while let Ok(message) = receiver.recv() {
                    match message {
                        Message::Event(mut event) => {
                            // An in-flight request may arrive after consent has
                            // been revoked. It cannot recreate the old path.
                            while let Some((time, key)) = expirations.front().copied() {
                                if time.elapsed() < Duration::from_secs(30 * 60) {
                                    break;
                                }
                                expirations.pop_front();
                                revoked.remove(&key);
                            }
                            if event
                                .subject
                                .is_some_and(|key| fail_closed || revoked.contains(&key))
                            {
                                event.subject = None;
                            }
                            let transition = sketch.record(event);
                            rollup.record(event, transition);
                            if event.subject.is_some() {
                                let _ = path_sink.try_record(event);
                            }
                        }
                        Message::Snapshot(reply) => {
                            let _ = reply.send(sketch.snapshot());
                        }
                        Message::Drain(reply) => {
                            let batch = std::mem::take(&mut rollup);
                            let _ = reply.send(batch);
                        }
                        Message::Forget(key, reply) => {
                            if revoked.insert(key) {
                                if revoked.len() > MAX_SUBJECTS {
                                    // Never sacrifice privacy to reduce memory.
                                    // Disable identified collection until restart.
                                    fail_closed = true;
                                    revoked.clear();
                                    expirations.clear();
                                } else {
                                    expirations.push_back((Instant::now(), key));
                                }
                            }
                            sketch.forget_subject(key);
                            // This sends behind all previously submitted path
                            // events; the reply confirms their deletion.
                            let deleted = path_sink.forget_for_privacy(key).is_some();
                            let _ = reply.send(deleted);
                        }
                    }
                }
            });
        Self {
            sender,
            dropped,
            paths,
            started: Instant::now(),
        }
    }

    pub fn uptime_millis(&self) -> u64 {
        self.started.elapsed().as_millis().min(u64::MAX as u128) as u64
    }

    pub fn try_record(&self, event: BehaviorEvent) -> bool {
        match self.sender.try_send(Message::Event(event)) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Diagnostics only, never called from HTTP request processing.
    /// A barrier event gives a consistent view of every earlier queued event.
    pub fn snapshot_for_diagnostics(&self) -> Option<SketchSnapshot> {
        let (reply, result) = mpsc::channel();
        self.sender.try_send(Message::Snapshot(reply)).ok()?;
        let snapshot = result.recv_timeout(Duration::from_secs(2)).ok()?;
        Some(snapshot.with_dropped(self.dropped.load(Ordering::Relaxed)))
    }

    /// Best-effort aggregation barrier. Only called from a background task,
    /// never on Axum's request executor. Failed persistence must retry the same
    /// batch ID and never replace the already-drained delta.
    pub fn drain_for_persistence(&self) -> Option<RollupDelta> {
        let (reply, response) = mpsc::channel();
        self.sender.try_send(Message::Drain(reply)).ok()?;
        response.recv_timeout(Duration::from_secs(2)).ok()
    }

    /// Serializes against earlier traffic events, then queries the independent
    /// offline path worker; never called from the HTTP finalizer.
    pub fn path_report_for_diagnostics(&self) -> Option<PathReport> {
        self.snapshot_for_diagnostics()?;
        self.paths.report_for_diagnostics()
    }

    /// Call only from an authenticated, CSRF-protected privacy action and a
    /// blocking executor. The result confirms both worker barriers, not an HTTP
    /// authorization or a global multi-instance database erasure.
    pub fn forget_subject_for_privacy(&self, key: VerifiedSubjectKey) -> Option<bool> {
        let (reply, answer) = mpsc::channel();
        self.sender.try_send(Message::Forget(key, reply)).ok()?;
        answer.recv_timeout(Duration::from_secs(2)).ok()
    }

    pub fn dropped_path_events(&self) -> u64 {
        self.paths.dropped_events()
    }

    pub fn dropped_events(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{LatencyGroup, RouteCode, StatusGroup};

    fn event() -> BehaviorEvent {
        BehaviorEvent {
            route: RouteCode::Catalog,
            status: StatusGroup::Success,
            latency: LatencyGroup::Under10Ms,
            subject: None,
            at_millis: 10,
        }
    }

    #[test]
    fn producer_is_nonblocking_and_diagnostics_snapshot_is_ordered() {
        let recorder = TrafficRecorder::start(5, 16);
        assert!(recorder.try_record(event()));
        assert!(recorder.try_record(event()));
        let snap = recorder.snapshot_for_diagnostics().expect("worker barrier");
        assert_eq!(snap.total, 2);
        assert_eq!(snap.count_route(RouteCode::Catalog), 2);
        assert_eq!(snap.active_subjects, 0);
        assert_eq!(snap.dropped_events, 0);
    }

    #[test]
    fn persistent_drain_resets_counters_but_not_subject_path() {
        use crate::domain::{RouteCode, VerifiedSubjectKey};
        let recorder = TrafficRecorder::start(5, 16);
        let mut first = event();
        first.subject = Some(VerifiedSubjectKey::from_hmac_digest([7; 16]));
        first.route = RouteCode::Catalog;
        assert!(recorder.try_record(first));
        let batch = recorder.drain_for_persistence().expect("first barrier");
        assert_eq!(batch.route_rows().len(), 1);
        assert!(batch.transition_rows().is_empty());
        assert!(recorder
            .drain_for_persistence()
            .expect("second barrier")
            .is_empty());
        let mut next = first;
        next.route = RouteCode::Books;
        next.at_millis = 300;
        assert!(recorder.try_record(next));
        let subsequent = recorder.drain_for_persistence().expect("path preserved");
        assert_eq!(subsequent.route_rows().len(), 1);
        assert_eq!(subsequent.transition_rows().len(), 1);
    }

    #[test]
    fn trusted_events_reach_independent_path_worker_without_http_sql() {
        use crate::domain::{RouteCode, VerifiedSubjectKey};
        let recorder = TrafficRecorder::start(8, 64);
        for i in 0..20 {
            let mut observed = event();
            observed.route = RouteCode::Unknown;
            observed.status = StatusGroup::ClientError;
            observed.subject = Some(VerifiedSubjectKey::from_hmac_digest([19; 16]));
            observed.at_millis = i * 100;
            assert!(recorder.try_record(observed));
        }
        let report = recorder
            .path_report_for_diagnostics()
            .expect("offline path worker");
        assert_eq!(report.assessed_events, 20);
        assert_eq!(report.active_subjects, 1);
        assert!(report.reject > 0);
        assert_eq!(recorder.dropped_path_events(), 0);
    }

    #[test]
    fn privacy_barrier_erases_subject_and_blocks_inflight_stale_events() {
        use crate::domain::{RouteCode, VerifiedSubjectKey};
        let recorder = TrafficRecorder::start(8, 64);
        let old = VerifiedSubjectKey::from_hmac_digest([41; 16]);
        let new_epoch = VerifiedSubjectKey::from_hmac_digest([42; 16]);
        let mut v = event();
        v.route = RouteCode::Books;
        v.subject = Some(old);
        assert!(recorder.try_record(v));
        assert_eq!(
            recorder
                .path_report_for_diagnostics()
                .unwrap()
                .active_subjects,
            1
        );
        assert_eq!(recorder.forget_subject_for_privacy(old), Some(true));
        assert_eq!(
            recorder
                .path_report_for_diagnostics()
                .unwrap()
                .active_subjects,
            0
        );
        // A request which started before opt-out cannot recreate the old path.
        assert!(recorder.try_record(v));
        let report = recorder.path_report_for_diagnostics().unwrap();
        assert_eq!(report.active_subjects, 0);
        assert_eq!(report.assessed_events, 1);
        // Re-enabling consent uses a different epoch-derived pseudonym.
        v.subject = Some(new_epoch);
        assert!(recorder.try_record(v));
        assert_eq!(
            recorder
                .path_report_for_diagnostics()
                .unwrap()
                .active_subjects,
            1
        );
    }

    #[test]
    fn full_queue_increments_drop_counter_without_blocking() {
        let (sender, _idle_receiver) = mpsc::sync_channel(1);
        let recorder = TrafficRecorder {
            sender,
            dropped: Arc::new(AtomicU64::new(0)),
            paths: PathRecorder::start(2, 2),
            started: Instant::now(),
        };
        assert!(recorder.try_record(event()));
        assert!(!recorder.try_record(event()));
        assert_eq!(recorder.dropped_events(), 1);
    }

    #[test]
    fn receiver_disconnect_does_not_break_primary_operation() {
        let (sender, receiver) = mpsc::sync_channel(1);
        drop(receiver);
        let recorder = TrafficRecorder {
            sender,
            dropped: Arc::new(AtomicU64::new(0)),
            paths: PathRecorder::start(2, 2),
            started: Instant::now(),
        };
        assert!(!recorder.try_record(event()));
        assert_eq!(recorder.dropped_events(), 1);
    }
}
