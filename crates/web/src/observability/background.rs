//! A causal child is a separate attempt, never an extension of HTTP latency.
use super::{new_trace_id, RequestTrace, TraceBuffer, TRACES};
use std::time::Instant;
use tor_observability::services::trace::background::{BackgroundTrace, Outcome};

/// Internal response metadata, produced only after a durable enqueue succeeds.
#[derive(Clone, Copy)]
pub(crate) struct QueuedWork;

pub(crate) fn retain_root(trace: &RequestTrace) {
    let retained = TRACES
        .get_or_init(TraceBuffer::default)
        .record_correlated(trace.clone());
    #[cfg(unix)]
    if retained {
        let _ = super::publisher().try_publish(trace);
    }
    #[cfg(not(unix))]
    let _ = retained;
}

/// Timing is wall time of sequential worker phases, not SQL execution or CPU.
/// Drop publishes interruption on cooperative cancellation. SIGKILL can lose
/// evidence; an absent receipt is explicitly not a successful attempt.
pub(crate) struct ImportSpan {
    record: Option<BackgroundTrace>,
    started: Instant,
    phase_started: Instant,
}

impl ImportSpan {
    pub(crate) fn new(parent: Option<u128>, attempt: i32) -> Self {
        let started = Instant::now();
        let record = parent.filter(|id| *id != 0).and_then(|parent| {
            let trace_id = new_trace_id().0;
            if !(1..=8).contains(&attempt) || trace_id == parent {
                return None;
            }
            Some(BackgroundTrace {
                parent_trace_id: parent,
                trace_id,
                attempt: attempt as u8,
                outcome: Outcome::Interrupted,
                phase: 0,
                duration_us: 0,
                phase_us: [0; 4],
            })
        });
        Self {
            record,
            started,
            phase_started: started,
        }
    }

    fn close_phase(&mut self, now: Instant) {
        if let Some(record) = &mut self.record {
            if (1..=4).contains(&record.phase) {
                let elapsed = now.duration_since(self.phase_started).as_micros();
                let index = usize::from(record.phase - 1);
                record.phase_us[index] = record.phase_us[index]
                    .saturating_add(elapsed.min(u64::MAX as u128) as u64);
            }
        }
        self.phase_started = now;
    }

    pub(crate) fn enter(&mut self, phase: u8) {
        let now = Instant::now();
        self.close_phase(now);
        if let Some(record) = &mut self.record {
            record.phase = phase;
        }
    }

    fn take_record(&mut self, outcome: Outcome) -> Option<BackgroundTrace> {
        let now = Instant::now();
        self.close_phase(now);
        let mut record = self.record.take()?;
        record.outcome = outcome;
        record.duration_us = now
            .duration_since(self.started)
            .as_micros()
            .min(u64::MAX as u128) as u64;
        Some(record)
    }

    pub(crate) fn finish(&mut self, outcome: Outcome) {
        if let Some(record) = self.take_record(outcome) {
            #[cfg(unix)]
            let _ = super::publisher().try_publish_background(&record);
            #[cfg(not(unix))]
            let _ = record;
        }
    }
}

impl Drop for ImportSpan {
    fn drop(&mut self) {
        self.finish(Outcome::Interrupted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_jobs_and_invalid_attempts_are_not_fabricated_as_correlated() {
        for (parent, attempt) in [(None, 1), (Some(0), 1), (Some(1), 0), (Some(1), 9)] {
            let mut span = ImportSpan::new(parent, attempt);
            span.enter(1);
            assert!(span.take_record(Outcome::Failed).is_none());
        }
    }

    #[test]
    fn phases_are_separate_and_finishing_is_once_only() {
        let mut span = ImportSpan::new(Some(16), 2);
        for phase in 1..=4 {
            span.enter(phase);
        }
        let record = span.take_record(Outcome::Completed).unwrap();
        assert_eq!(record.parent_trace_id, 16);
        assert_ne!(record.trace_id, 16);
        assert_eq!(record.attempt, 2);
        assert_eq!(record.phase, 4);
        assert!(record.encode().is_some());
        assert!(span.take_record(Outcome::Interrupted).is_none());
    }

    #[test]
    fn unfinished_work_retains_last_phase_without_claiming_success() {
        let mut span = ImportSpan::new(Some(16), 1);
        span.enter(2);
        let record = span.take_record(Outcome::Interrupted).unwrap();
        assert_eq!(record.phase, 2);
        assert_eq!(record.outcome, Outcome::Interrupted);
        assert!(record.encode().is_some());
    }
}
