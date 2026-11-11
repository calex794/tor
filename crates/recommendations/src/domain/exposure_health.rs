//! Durable evidence for registered observation workers; not installation-wide traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ExposureHealthPeriod {
    pub confirmed_hours: i32,
    pub loss_hours: i32,
    pub error_hours: i32,
}
impl ExposureHealthPeriod {
    pub fn has_failure(self) -> bool {
        self.loss_hours > 0 || self.error_hours > 0
    }
    pub fn complete(self, days: i32) -> bool {
        [1, 7, 30, 90].contains(&days) && self.confirmed_hours == days * 24 && !self.has_failure()
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExposureHealth {
    pub until_epoch: i64,
    pub current: ExposureHealthPeriod,
    pub previous: ExposureHealthPeriod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HealthCounters {
    pub dropped: i64,
    pub failed: i64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthLease {
    pub epoch: i64,
    pub clock_ms: i64,
    pub lease_until_ms: i64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthCut {
    pub lease: HealthLease,
    pub cut_ms: i64,
}
