//! Bounded, anonymous, process-local request traces. Not a security audit log.
use crate::domain::{RouteGroup, StatusGroup};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};

pub const MAX_TRACES: usize = 128;
pub const MAX_SQL_SPANS: usize = 8;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SqlSpan {
    /// Completion time relative to the beginning of the HTTP handler.
    pub completed_at_us: u64,
    /// Client-side SQLx elapsed time; not PostgreSQL execution CPU time.
    pub elapsed_us: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqlEvidence {
    pub observed_queries: u64,
    pub observed_elapsed_us: u64,
    pub spans: Vec<SqlSpan>,
    pub uncaptured_spans: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RequestTrace {
    pub trace_id: u128,
    pub route: RouteGroup,
    pub status: StatusGroup,
    pub duration_us: u64,
    /// None means SQLx observation was not installed, NOT zero queries.
    pub sqlx: Option<SqlEvidence>,
}

impl RequestTrace {
    fn should_keep(&self) -> bool {
        // Tail decision once for the ENTIRE trace, never for each SQL span.
        self.status == StatusGroup::ServerError
            || self.duration_us >= 1_000_000
            || self.trace_id.is_multiple_of(16)
    }
}

/// All retained traces are complete at the HTTP boundary. The queue is bounded
/// and never blocks a request on network or filesystem I/O.
#[derive(Default)]
pub struct TraceBuffer {
    recent: Mutex<VecDeque<RequestTrace>>,
    evicted: AtomicU64,
}

impl TraceBuffer {
    pub fn record(&self, trace: RequestTrace) -> bool {
        if !trace.should_keep() {
            return false;
        }
        let mut recent = self
            .recent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if recent.len() == MAX_TRACES {
            recent.pop_front();
            self.evicted.fetch_add(1, Ordering::Relaxed);
        }
        recent.push_back(trace);
        true
    }

    /// Operator-only in-process diagnostics; no public HTTP route.
    pub fn snapshot(&self) -> Vec<RequestTrace> {
        self.recent
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    pub fn evicted(&self) -> u64 {
        self.evicted.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trace(id: u128, status: StatusGroup, duration_us: u64) -> RequestTrace {
        RequestTrace {
            trace_id: id,
            route: RouteGroup::Catalog,
            status,
            duration_us,
            sqlx: Some(SqlEvidence {
                observed_queries: 2,
                observed_elapsed_us: 7_000,
                spans: vec![
                    SqlSpan {
                        completed_at_us: 3_000,
                        elapsed_us: 2_000,
                    },
                    SqlSpan {
                        completed_at_us: 9_000,
                        elapsed_us: 5_000,
                    },
                ],
                uncaptured_spans: 0,
            }),
        }
    }

    #[test]
    fn decision_is_once_for_a_complete_request() {
        let buffer = TraceBuffer::default();
        assert!(!buffer.record(trace(1, StatusGroup::Success, 500)));
        assert!(buffer.record(trace(16, StatusGroup::Success, 500)));
        assert!(buffer.record(trace(17, StatusGroup::ServerError, 500)));
        assert!(buffer.record(trace(18, StatusGroup::Success, 1_000_000)));
        let retained = buffer.snapshot();
        assert_eq!(retained.len(), 3);
        assert_eq!(retained[0].sqlx.as_ref().unwrap().spans.len(), 2);
        assert_eq!(retained[2].trace_id, 18);
    }

    #[test]
    fn ring_is_bounded_and_reports_eviction() {
        let buffer = TraceBuffer::default();
        for id in 1..=200 {
            assert!(buffer.record(trace(id, StatusGroup::ServerError, 100)));
        }
        assert_eq!(buffer.snapshot().len(), MAX_TRACES);
        assert_eq!(buffer.snapshot()[0].trace_id, 73);
        assert_eq!(buffer.evicted(), 72);
    }

    #[test]
    fn no_sqlx_observer_is_unknown_not_measured_zero() {
        let buffer = TraceBuffer::default();
        let mut value = trace(16, StatusGroup::Success, 10);
        value.sqlx = None;
        assert!(buffer.record(value));
        assert!(buffer.snapshot()[0].sqlx.is_none());
    }
}
