//! Lifetime-scoped numeric child evidence. On timeout/cancel/panic Drop records
//! interrupted, never success. Fenced SQL state remains authoritative.
use super::ChapterImportWorkerResult;
use std::time::Instant;
use tor_imports::domain::ChapterImportClaim;
use tor_observability::services::trace::background::{BackgroundTrace, Outcome};

pub(super) struct AttemptTrace {
    parent: Option<u128>,
    child: u128,
    attempt: u8,
    started: Instant,
    phase_started: Instant,
    phase: u8,
    phase_us: [u64; 4],
    outcome: Outcome,
}

impl AttemptTrace {
    pub(super) fn new(claim: &ChapterImportClaim) -> Self {
        let now = Instant::now();
        Self {
            parent: claim.parent_trace_id,
            child: crate::observability::new_trace_id().0,
            attempt: u8::try_from(claim.attempts).unwrap_or(0),
            started: now,
            phase_started: now,
            phase: 0,
            phase_us: [0; 4],
            outcome: Outcome::Interrupted,
        }
    }

    fn charge(&mut self) {
        if (1..=4).contains(&self.phase) {
            let index = usize::from(self.phase - 1);
            self.phase_us[index] = self.phase_us[index].saturating_add(
                self.phase_started.elapsed().as_micros().min(u64::MAX as u128) as u64,
            );
        }
    }

    pub(super) fn enter(&mut self, phase: u8) {
        self.charge();
        self.phase = phase;
        self.phase_started = Instant::now();
    }

    pub(super) fn finish(&mut self, result: &Result<ChapterImportWorkerResult, &'static str>) {
        self.outcome = match result {
            Ok(ChapterImportWorkerResult::Completed) => Outcome::Completed,
            Ok(ChapterImportWorkerResult::Retried) => Outcome::Retried,
            Ok(ChapterImportWorkerResult::Failed) => Outcome::Failed,
            Ok(ChapterImportWorkerResult::Stale) => Outcome::Stale,
            Ok(ChapterImportWorkerResult::Idle) | Err(_) => Outcome::Unavailable,
        };
    }
}

impl Drop for AttemptTrace {
    fn drop(&mut self) {
        self.charge();
        let Some(parent_trace_id) = self.parent else {
            return;
        };
        let trace = BackgroundTrace {
            parent_trace_id,
            trace_id: self.child,
            attempt: self.attempt,
            outcome: self.outcome,
            phase: self.phase,
            duration_us: self.started.elapsed().as_micros().min(u64::MAX as u128) as u64,
            phase_us: self.phase_us,
        };
        crate::observability::publish_background_trace(&trace);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_and_fenced_failure_are_distinct() {
        let dummy = ChapterImportClaim {
            job_id: 1,
            parent_trace_id: Some(16),
            book_id: 2,
            actor_id: 3,
            command_key: String::new(),
            source_key: String::new(),
            source_filename: String::new(),
            source_sha256: Vec::new(),
            source_bytes: 0,
            approved_fingerprint: Vec::new(),
            expected_chapter_count: 0,
            attempts: 1,
            epoch: 1,
        };
        let mut trace = AttemptTrace::new(&dummy);
        trace.enter(1);
        trace.enter(2);
        trace.enter(3);
        trace.enter(4);
        trace.finish(&Ok(ChapterImportWorkerResult::Completed));
        assert_eq!(trace.outcome, Outcome::Completed);
        assert!(BackgroundTrace {
            parent_trace_id: 16,
            trace_id: trace.child,
            attempt: 1,
            outcome: trace.outcome,
            phase: trace.phase,
            duration_us: trace.started.elapsed().as_micros().min(u64::MAX as u128) as u64,
            phase_us: trace.phase_us,
        }.encode().is_some());
        trace.finish(&Ok(ChapterImportWorkerResult::Stale));
        assert_eq!(trace.outcome, Outcome::Stale);
        trace.finish(&Err("job_update_unavailable"));
        assert_eq!(trace.outcome, Outcome::Unavailable);
    }
}
