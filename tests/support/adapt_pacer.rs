//! Shared experimental policy, compiled only in disposable database tests.
use std::time::Duration;

pub(crate) const MIN_MS: u64 = 250;
pub(crate) const MAX_MS: u64 = 5000;

#[derive(Clone, Copy, Debug)]
pub(crate) enum Policy {
    Baseline,
    AgeAware,
    ProgressBudget,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Pressure {
    Missing,
    High,
    Low,
    Neutral,
}

pub(crate) struct Pacer {
    policy: Policy,
    delay: u64,
    healthy: u8,
}

impl Pacer {
    pub(crate) fn new(policy: Policy) -> Self {
        Self {
            policy,
            delay: MIN_MS,
            healthy: 0,
        }
    }

    pub(crate) fn after_progress(
        &mut self,
        pressure: Pressure,
        pending: i64,
        age: Duration,
    ) -> u64 {
        if matches!(self.policy, Policy::Baseline) {
            return MIN_MS;
        }
        match pressure {
            Pressure::Missing => {
                self.delay = MAX_MS;
                self.healthy = 0;
            }
            Pressure::High => {
                let ceiling = if matches!(self.policy, Policy::ProgressBudget) {
                    self.delay.max(1000)
                } else {
                    MAX_MS
                };
                self.delay = (2 * self.delay).min(ceiling);
                self.healthy = 0;
            }
            Pressure::Low => {
                self.healthy += 1;
                if pending >= 8 || age >= Duration::from_secs(8) {
                    self.delay = MIN_MS;
                    self.healthy = 0;
                } else if self.healthy >= 2 {
                    self.delay = (self.delay / 2).max(MIN_MS);
                    self.healthy = 0;
                }
            }
            Pressure::Neutral => self.healthy = 0,
        }
        assert!((MIN_MS..=MAX_MS).contains(&self.delay));
        self.delay
    }
}

pub(crate) fn percentile(mut values: Vec<u64>, p: usize) -> Option<u64> {
    values.sort_unstable();
    let rank = (values.len() * p).div_ceil(100);
    rank.checked_sub(1).and_then(|i| values.get(i).copied())
}

pub(crate) fn classify(reader: Vec<Option<u64>>, probe: Vec<Option<u64>>) -> Pressure {
    if reader.is_empty() || probe.is_empty() {
        return Pressure::Missing;
    }
    if reader.iter().chain(&probe).any(Option::is_none) {
        return Pressure::High;
    }
    let reader_p95 = percentile(reader.into_iter().flatten().collect(), 95).unwrap();
    let probe_p95 = percentile(probe.into_iter().flatten().collect(), 95).unwrap();
    if reader_p95 >= 100_000 || probe_p95 >= 50_000 {
        Pressure::High
    } else if reader_p95 <= 50_000 && probe_p95 <= 10_000 {
        Pressure::Low
    } else {
        Pressure::Neutral
    }
}

#[test]
fn pacing_policies_distinguish_high_pressure_and_recover_safely() {
    let mut baseline = Pacer::new(Policy::Baseline);
    let mut age_aware = Pacer::new(Policy::AgeAware);
    let mut budget = Pacer::new(Policy::ProgressBudget);
    let age = Duration::from_secs(8);
    for _ in 0..20 {
        let delay = baseline.after_progress(Pressure::High, 100_000, age);
        assert_eq!(delay, MIN_MS);
        age_aware.after_progress(Pressure::High, 100_000, age);
        budget.after_progress(Pressure::High, 100_000, age);
    }
    let delay = age_aware.after_progress(Pressure::High, 100_000, age);
    assert_eq!(delay, MAX_MS);
    assert_eq!(budget.after_progress(Pressure::High, 100_000, age), 1000);
    for mut pacer in [age_aware, budget] {
        let delay = pacer.after_progress(Pressure::Missing, 100_000, age);
        assert_eq!(delay, MAX_MS);
        assert_eq!(pacer.after_progress(Pressure::High, 100_000, age), MAX_MS);
        let delay = pacer.after_progress(Pressure::Neutral, 100_000, age);
        assert_eq!(delay, MAX_MS);
        assert_eq!(pacer.after_progress(Pressure::Low, 100_000, age), MIN_MS);
    }
}
