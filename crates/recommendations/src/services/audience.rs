//! F-owned discovery totals. These are consented first-book facts, not visitors
//! or C's chapter counters; an opening/start ratio is not a funnel conversion.
use std::{error::Error, fmt};

pub mod books;

pub const DISCOVERY_PERIOD_DAYS: [i32; 4] = [1, 7, 30, 90];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiscoveryError {
    Invalid,
    Unavailable,
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl Error for DiscoveryError {}

pub fn validate_period(days: i32) -> Result<(), DiscoveryError> {
    if DISCOVERY_PERIOD_DAYS.contains(&days) {
        Ok(())
    } else {
        Err(DiscoveryError::Invalid)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiscoverySource {
    Catalog,
    Search,
    Recommendations,
    /// Canonical first reads without a qualifying navigation; not a source.
    Unattributed,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryChannel {
    pub source: DiscoverySource,
    /// None means not measured, missing aggregate evidence, or incomplete
    /// historical coverage. A measured zero is always Some(0).
    pub clicks: Option<i64>,
    pub book_opens: Option<i64>,
    pub starts: Option<i64>,
    pub completions: Option<i64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryPeriod {
    /// UTC calendar dates, inclusive start and exclusive end (YYYY-MM-DD).
    /// Navigation sources use the assignment/touch cohort day; unattributed
    /// reads use the canonical event day. Current today is always unfinished.
    pub from_day: String,
    pub until_day_exclusive: String,
    pub includes_partial_today: bool,
    /// Exactly catalog, search, recommendations, unattributed, in this order.
    pub channels: Vec<DiscoveryChannel>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryHealth {
    pub last_success_utc: Option<String>,
    pub fresh: bool,
    pub rec_dirty: bool,
    pub organic_dirty: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryReport {
    pub days: i32,
    pub measured_at_utc: String,
    pub current: DiscoveryPeriod,
    pub previous: DiscoveryPeriod,
    pub health: DiscoveryHealth,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepted_periods_fit_two_adjacent_retained_windows() {
        for days in DISCOVERY_PERIOD_DAYS {
            assert_eq!(validate_period(days), Ok(()));
            assert!(days * 2 <= 180);
        }
        for days in [i32::MIN, -1, 0, 2, 31, 91, i32::MAX] {
            assert_eq!(validate_period(days), Err(DiscoveryError::Invalid));
        }
    }
}
