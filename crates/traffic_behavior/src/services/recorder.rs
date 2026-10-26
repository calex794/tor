//! Never block HTTP on analytics. A single bounded worker drains compact events.
//! Full/disconnected queues count dropped telemetry, not failed user requests.
use crate::domain::{BehaviorEvent, BehaviorSketch, SketchSnapshot};
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
}

/// Cloneable, request-safe telemetry sender. No raw path or identity fields.
#[derive(Clone)]
pub struct TrafficRecorder {
    sender: SyncSender<Message>,
    dropped: Arc<AtomicU64>,
    started: Instant,
}

impl TrafficRecorder {
    pub fn start(max_subjects: usize, queue_capacity: usize) -> Self {
        let (sender, receiver) = mpsc::sync_channel(queue_capacity.clamp(1, MAX_QUEUE));
        let dropped = Arc::new(AtomicU64::new(0));
        // Thread creation failure only disables best-effort telemetry.
        let _ = thread::Builder::new()
            .name("tor-ril-traffic".to_owned())
            .spawn(move || {
                let mut sketch = BehaviorSketch::new(max_subjects);
                while let Ok(message) = receiver.recv() {
                    match message {
                        Message::Event(event) => sketch.record(event),
                        Message::Snapshot(reply) => {
                            let _ = reply.send(sketch.snapshot());
                        }
                    }
                }
            });
        Self {
            sender,
            dropped,
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
    fn full_queue_increments_drop_counter_without_blocking() {
        let (sender, _idle_receiver) = mpsc::sync_channel(1);
        let recorder = TrafficRecorder {
            sender,
            dropped: Arc::new(AtomicU64::new(0)),
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
            started: Instant::now(),
        };
        assert!(!recorder.try_record(event()));
        assert_eq!(recorder.dropped_events(), 1);
    }
}
