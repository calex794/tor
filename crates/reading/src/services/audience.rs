//! C-owned numerical reading evidence. No visitor profile or attribution is built.
use std::{error::Error, fmt};

pub const AUDIENCE_PERIOD_DAYS: [i32; 4] = [1, 7, 30, 90];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AudienceError {
    Invalid,
    Unavailable,
}

impl fmt::Display for AudienceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl Error for AudienceError {}

pub fn validate_period(days: i32) -> Result<(), AudienceError> {
    if AUDIENCE_PERIOD_DAYS.contains(&days) {
        Ok(())
    } else {
        Err(AudienceError::Invalid)
    }
}

/// First canonical chapter events within one interval, not visits or daily activity.
/// The same account is counted once across BOTH event kinds and the ENTIRE interval.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AudienceCounts {
    pub starts: i64,
    pub completions: i64,
    pub unique_readers: i64,
    pub unique_started: i64,
    pub unique_completed: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudiencePeriod {
    /// Inclusive start and exclusive end, UTC RFC3339.
    pub from_utc: String,
    pub until_utc: String,
    pub available: bool,
    /// No matching C facts is not proof that there were no visitors.
    pub counts: Option<AudienceCounts>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudienceReport {
    pub days: i32,
    pub measured_at_utc: String,
    pub current: AudiencePeriod,
    pub previous: AudiencePeriod,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_bounded_product_periods_are_accepted() {
        for days in AUDIENCE_PERIOD_DAYS {
            assert_eq!(validate_period(days), Ok(()));
        }
        for days in [i32::MIN, -1, 0, 2, 31, 91, i32::MAX] {
            assert_eq!(validate_period(days), Err(AudienceError::Invalid));
        }
    }
}
