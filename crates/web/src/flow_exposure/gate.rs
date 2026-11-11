use super::*;
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use tor_recommendations::domain::exposure_health::HealthLease;

#[derive(Default)]
pub(super) struct Gate {
    lease: Mutex<Option<(i64, Instant)>>,
}
impl Gate {
    pub(super) fn epoch(&self) -> Option<i64> {
        self.epoch_at(Instant::now())
    }
    fn epoch_at(&self, now: Instant) -> Option<i64> {
        self.lease
            .lock()
            .ok()?
            .as_ref()
            .filter(|(_, until)| now < *until)
            .map(|(epoch, _)| *epoch)
    }
    pub(super) fn renew(&self, lease: HealthLease, started: Instant) -> bool {
        let remaining = lease
            .lease_until_ms
            .saturating_sub(lease.clock_ms)
            .saturating_sub(5000);
        let Ok(mut value) = self.lease.lock() else {
            return false;
        };
        if remaining <= 0 {
            *value = None;
            return false;
        }
        let deadline = started + Duration::from_millis(remaining.min(25_000) as u64);
        *value = (Instant::now() < deadline).then_some((lease.epoch, deadline));
        value.is_some()
    }
    pub(super) fn disable(&self) {
        if let Ok(mut value) = self.lease.lock() {
            *value = None;
        }
    }
}

#[derive(Clone)]
pub struct QueueProbe {
    pub(super) counters: Arc<Counters>,
    pub(super) gate: Arc<Gate>,
}
impl QueueProbe {
    /// Current process diagnostics only. Durable window evidence is F-owned SQL.
    pub fn snapshot(&self) -> QueueSnapshot {
        let mut snapshot = self.counters.snapshot();
        snapshot.ready = self.gate.epoch().is_some();
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parked_gate_expires_by_monotonic_time_without_a_timer_task() {
        let now = Instant::now();
        let gate = Gate::default();
        assert!(gate.renew(
            HealthLease {
                epoch: 7,
                clock_ms: 1000,
                lease_until_ms: 31000
            },
            now
        ));
        assert_eq!(gate.epoch_at(now + Duration::from_millis(24_999)), Some(7));
        assert_eq!(gate.epoch_at(now + Duration::from_secs(25)), None);
        assert!(!gate.renew(
            HealthLease {
                epoch: 7,
                clock_ms: 31000,
                lease_until_ms: 31000
            },
            now
        ));
    }
    #[test]
    fn rejected_issuance_counts_a_loss_even_before_first_ready_epoch() {
        let (sender, _receiver) = mpsc::channel(1);
        let counters = Arc::new(Counters::default());
        let issuer = Issuer {
            sender,
            counters: counters.clone(),
            gate: Arc::new(Gate::default()),
        };
        assert!(issuer
            .issue(CardObservation {
                book_id: 1,
                surface: tor_contracts::exposure::ExposureSurface::Home,
                editorial: None
            })
            .is_none());
        assert_eq!(counters.dropped.load(Ordering::Acquire), 1);
        assert_eq!(counters.queued.load(Ordering::Acquire), 0);
    }
}
