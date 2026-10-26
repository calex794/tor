//! Off-path RIL-07 worker. Bound queue, single consumer, no SQL or raw IDs.
//! No blocking work or path scoring happens inside an Axum request.
use crate::domain::{BehaviorEvent, PathAnalyzer, PathReport, VerifiedSubjectKey};
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender, TrySendError},
        Arc,
    },
    thread,
    time::Duration,
};

const MAX_PATH_QUEUE: usize = 4_096;

enum Message {
    Event(BehaviorEvent),
    Report(mpsc::Sender<PathReport>),
    Forget(VerifiedSubjectKey, mpsc::Sender<bool>),
}

#[derive(Clone)]
pub struct PathRecorder {
    sender: SyncSender<Message>,
    dropped: Arc<AtomicU64>,
}

impl PathRecorder {
    pub fn start(max_subjects: usize, queue_capacity: usize) -> Self {
        let (sender, receiver) = mpsc::sync_channel(queue_capacity.clamp(1, MAX_PATH_QUEUE));
        let dropped = Arc::new(AtomicU64::new(0));
        let _ = thread::Builder::new()
            .name("tor-ril-paths".to_owned())
            .spawn(move || {
                let mut analyzer = PathAnalyzer::new(max_subjects);
                while let Ok(message) = receiver.recv() {
                    match message {
                        Message::Event(event) => {
                            let _ = analyzer.record(event);
                        }
                        Message::Report(reply) => {
                            let _ = reply.send(analyzer.report());
                        }
                        Message::Forget(key, reply) => {
                            let _ = reply.send(analyzer.forget_subject(key));
                        }
                    }
                }
            });
        Self { sender, dropped }
    }

    pub fn try_record(&self, event: BehaviorEvent) -> bool {
        // Never link separate anonymous requests to an invented identity.
        if event.subject.is_none() {
            return false;
        }
        match self.sender.try_send(Message::Event(event)) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Diagnostics only: aggregate counters without keys or path tokens.
    /// Call only from a non-HTTP diagnostics context.
    pub fn report_for_diagnostics(&self) -> Option<PathReport> {
        let (reply, receiver) = mpsc::channel();
        self.sender.try_send(Message::Report(reply)).ok()?;
        let report = receiver.recv_timeout(Duration::from_secs(2)).ok()?;
        Some(report.with_dropped(self.dropped.load(Ordering::Relaxed)))
    }

    /// A trusted, non-HTTP privacy worker can request removal. Queue overload
    /// returns None (NOT "success"); the caller must retry. Not wired to auth yet.
    pub fn forget_for_privacy(&self, key: VerifiedSubjectKey) -> Option<bool> {
        let (reply, receiver) = mpsc::channel();
        self.sender.try_send(Message::Forget(key, reply)).ok()?;
        receiver.recv_timeout(Duration::from_secs(2)).ok()
    }

    pub fn dropped_events(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{Hygiene, LatencyGroup, RouteCode, StatusGroup};

    fn verified(i: u8, route: RouteCode, time: u64) -> BehaviorEvent {
        BehaviorEvent {
            route,
            status: StatusGroup::ClientError,
            latency: LatencyGroup::Under10Ms,
            subject: Some(VerifiedSubjectKey::from_hmac_digest([i; 16])),
            at_millis: time,
        }
    }

    #[test]
    fn worker_is_off_path_and_reports_only_anonymous_aggregates() {
        let worker = PathRecorder::start(8, 64);
        for i in 0..20 {
            assert!(worker.try_record(verified(11, RouteCode::Unknown, i * 50)));
        }
        let report = worker.report_for_diagnostics().expect("worker barrier");
        assert_eq!(report.assessed_events, 20);
        assert!(report.reject > 0);
        assert_eq!(report.active_subjects, 1);
        let formatted = format!("{report:?}");
        assert!(!formatted.contains("11, 11"));
        assert!(!formatted.contains("subject"));
        assert!(!formatted.contains("user"));
        assert_eq!(Hygiene::Normal as u8, 0);
    }

    #[test]
    fn anonymous_events_never_form_paths_or_count_as_dropped() {
        let worker = PathRecorder::start(8, 4);
        let mut event = verified(2, RouteCode::Unknown, 4);
        event.subject = None;
        for _ in 0..1000 {
            assert!(!worker.try_record(event));
        }
        let report = worker.report_for_diagnostics().expect("barrier");
        assert_eq!(report.assessed_events, 0);
        assert_eq!(report.dropped_events, 0);
    }

    #[test]
    fn privacy_removal_clears_active_path_and_new_events_start_fresh() {
        let worker = PathRecorder::start(8, 8);
        assert!(worker.try_record(verified(7, RouteCode::Books, 10)));
        assert_eq!(worker.forget_for_privacy(VerifiedSubjectKey::from_hmac_digest([7; 16])), Some(true));
        assert!(worker.try_record(verified(7, RouteCode::Read, 20)));
        let report = worker.report_for_diagnostics().unwrap();
        assert_eq!(report.assessed_events, 2);
        assert_eq!(report.active_subjects, 1);
    }

    #[test]
    fn saturation_is_counted_without_panicking_or_blocking() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let worker = PathRecorder { sender, dropped: Arc::new(AtomicU64::new(0)) };
        assert!(worker.try_record(verified(4, RouteCode::Books, 1)));
        assert!(!worker.try_record(verified(4, RouteCode::Books, 2)));
        assert_eq!(worker.dropped_events(), 1);
        assert!(worker.report_for_diagnostics().is_none());
    }

    #[test]
    fn lost_worker_does_not_affect_sending_site_requests() {
        let (sender, receiver) = mpsc::sync_channel(1);
        drop(receiver);
        let worker = PathRecorder { sender, dropped: Arc::new(AtomicU64::new(0)) };
        assert!(!worker.try_record(verified(4, RouteCode::Reader, 1)));
        assert_eq!(worker.dropped_events(), 1);
    }
}
