//! Short nonblocking producer/barrier fence. No database work or waiting occurs
//! while held; a paused producer cannot enqueue behind an acknowledged barrier.
use crate::domain::html_health::HTML_CLOCK_MARGIN_MS;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex, MutexGuard,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub(super) struct FlowGate {
    created: Instant,
    managed: AtomicBool,
    ready: AtomicBool,
    state: Mutex<Option<ClockLease>>,
}
impl Default for FlowGate {
    fn default() -> Self {
        Self {
            created: Instant::now(),
            managed: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            state: Mutex::new(None),
        }
    }
}
struct ClockLease {
    started: Instant,
    finished: Instant,
    db_ms: i64,
}
impl FlowGate {
    pub(super) fn created(&self) -> Instant {
        self.created
    }
    pub(super) fn manage(&self) {
        self.managed.store(true, Ordering::Release);
    }
    pub(super) fn lock(&self) -> Option<FlowGuard<'_>> {
        let state = self.state.try_lock().ok()?;
        Some(FlowGuard {
            state,
            managed: self.managed.load(Ordering::Acquire),
            ready: &self.ready,
        })
    }
    pub(super) fn activate(&self, db_ms: i64, started: Instant) -> bool {
        let finished = Instant::now();
        let Some(elapsed) = finished.checked_duration_since(started) else {
            return false;
        };
        // The DB sample is floor-rounded to milliseconds. Its possible interval
        // must fit inside both sides of the 2s clock margin.
        if db_ms <= 0
            || elapsed >= Duration::from_secs(25)
            || elapsed.saturating_add(Duration::from_millis(1))
                > Duration::from_millis((2 * HTML_CLOCK_MARGIN_MS) as u64)
        {
            return false;
        }
        let Ok(mut state) = self.state.try_lock() else {
            return false;
        };
        *state = Some(ClockLease {
            started,
            finished,
            db_ms,
        });
        self.ready.store(true, Ordering::Release);
        true
    }
    pub(super) fn close(&self) {
        self.ready.store(false, Ordering::Release);
        if let Ok(mut state) = self.state.try_lock() {
            *state = None;
        }
    }
}
pub(super) struct FlowGuard<'a> {
    state: MutexGuard<'a, Option<ClockLease>>,
    managed: bool,
    ready: &'a AtomicBool,
}
impl FlowGuard<'_> {
    pub(super) fn observation_time(&self) -> Option<SystemTime> {
        let before = Instant::now();
        let now = SystemTime::now();
        let after = Instant::now();
        if !self.managed {
            return Some(now);
        }
        if !self.ready.load(Ordering::Acquire) {
            return None;
        }
        let lease = self.state.as_ref()?;
        let earliest = before.checked_duration_since(lease.finished)?;
        let latest = after.checked_duration_since(lease.started)?;
        (latest < Duration::from_secs(25)
            && clock_accepts(
                now.duration_since(UNIX_EPOCH).ok()?,
                lease.db_ms,
                earliest,
                latest,
            ))
        .then_some(now)
    }
}

/// The DB sample occurred somewhere between SQL start and completion, and the
/// system-clock read somewhere between its surrounding monotonic samples.
/// Accept only the intersection of all possible +/- margin intervals.
fn clock_accepts(
    actual: Duration,
    db_ms: i64,
    earliest_elapsed: Duration,
    latest_elapsed: Duration,
) -> bool {
    if db_ms <= 0 || earliest_elapsed > latest_elapsed {
        return false;
    }
    let (Ok(actual), Ok(earliest), Ok(latest)) = (
        i128::try_from(actual.as_nanos()),
        i128::try_from(earliest_elapsed.as_nanos()),
        i128::try_from(latest_elapsed.as_nanos()),
    ) else {
        return false;
    };
    const NANOS_PER_MS: i128 = 1_000_000;
    let lower = i128::from(db_ms) * NANOS_PER_MS + earliest;
    let upper = (i128::from(db_ms) + 1) * NANOS_PER_MS + latest;
    let margin = i128::from(HTML_CLOCK_MARGIN_MS) * NANOS_PER_MS;
    upper - margin <= actual && actual <= lower + margin
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lease_and_clock_are_checked_even_when_a_producer_keeps_the_guard() {
        let gate = FlowGate::default();
        gate.manage();
        assert!(gate.lock().unwrap().observation_time().is_none());
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64;
        assert!(gate.activate(now, Instant::now()));
        let guard = gate.lock().unwrap();
        assert!(guard.observation_time().is_some());
        assert!(gate.lock().is_none());
        drop(guard);
        assert!(gate.activate(now - 10_000, Instant::now()));
        assert!(gate.lock().unwrap().observation_time().is_none());
        assert!(!gate.activate(now, Instant::now() - Duration::from_secs(25)));
        assert!(!gate.activate(now, Instant::now() - Duration::from_millis(4_001)));
        gate.close();
        assert!(gate.lock().unwrap().observation_time().is_none());
    }

    #[test]
    fn rtt_uncertainty_tightens_both_clock_boundaries() {
        let db = 3_599_900;
        let earliest = Duration::from_millis(100);
        let latest = Duration::from_millis(1_000);
        // DB now may be anywhere in [3_600_000, 3_600_901) ms.
        // The intersection for a +/- 2s guarantee is [3_598_901, 3_602_000].
        assert!(clock_accepts(
            Duration::from_millis(3_598_901),
            db,
            earliest,
            latest
        ));
        assert!(clock_accepts(
            Duration::from_millis(3_602_000),
            db,
            earliest,
            latest
        ));
        assert!(!clock_accepts(
            Duration::from_millis(3_598_901) - Duration::from_nanos(1),
            db,
            earliest,
            latest,
        ));
        assert!(!clock_accepts(
            Duration::from_millis(3_602_000) + Duration::from_nanos(1),
            db,
            earliest,
            latest,
        ));
        // The previous point estimate accepted this 2.9s positive skew.
        assert!(!clock_accepts(
            Duration::from_millis(3_602_900),
            db,
            earliest,
            latest
        ));
        assert!(!clock_accepts(
            Duration::from_millis(3_601_900),
            db,
            Duration::ZERO,
            Duration::from_millis(4_001),
        ));
    }

    #[test]
    fn positive_and_negative_offsets_and_the_rtt_limit_are_explicit() {
        let db = 10_000;
        assert!(clock_accepts(
            Duration::from_millis(8_001),
            db,
            Duration::ZERO,
            Duration::ZERO
        ));
        assert!(!clock_accepts(
            Duration::from_millis(8_000),
            db,
            Duration::ZERO,
            Duration::ZERO
        ));
        assert!(clock_accepts(
            Duration::from_millis(12_000),
            db,
            Duration::ZERO,
            Duration::ZERO
        ));
        assert!(!clock_accepts(
            Duration::from_millis(12_000) + Duration::from_nanos(1),
            db,
            Duration::ZERO,
            Duration::ZERO
        ));
        // 3999ms plus the 1ms DB-floor uncertainty leaves exactly one safe point.
        assert!(clock_accepts(
            Duration::from_millis(12_000),
            db,
            Duration::ZERO,
            Duration::from_millis(3_999)
        ));
        assert!(!clock_accepts(
            Duration::from_millis(12_000),
            db,
            Duration::ZERO,
            Duration::from_millis(4_000)
        ));
        assert!(!clock_accepts(
            Duration::from_millis(12_000),
            db,
            Duration::ZERO,
            Duration::from_millis(4_001)
        ));
    }

    #[test]
    fn a_loss_cannot_cross_beyond_the_inclusive_db_margin_hour() {
        const HOUR: i64 = 3_600_000;
        let db = HOUR - HTML_CLOCK_MARGIN_MS - 1;
        let old_accepted = HOUR + 899;
        assert!(!clock_accepts(
            Duration::from_millis(old_accepted as u64),
            db,
            Duration::ZERO,
            Duration::from_millis(900),
        ));
        let latest_allowed = db + HTML_CLOCK_MARGIN_MS;
        assert!(clock_accepts(
            Duration::from_millis(latest_allowed as u64),
            db,
            Duration::ZERO,
            Duration::from_millis(900),
        ));
        assert_eq!(latest_allowed.div_euclid(HOUR), 0);
        // Equality at the UTC boundary belongs to the next hour; flag SQL must
        // retain its inclusive upper bucket rather than subtracting one ms.
        let boundary_db = HOUR - HTML_CLOCK_MARGIN_MS;
        assert!(clock_accepts(
            Duration::from_millis(HOUR as u64),
            boundary_db,
            Duration::ZERO,
            Duration::from_millis(900),
        ));
        assert_eq!((boundary_db + HTML_CLOCK_MARGIN_MS).div_euclid(HOUR), 1);
    }
}
