//! Administrative return-to-reading counts from consented, accepted C progress.
//! A cohort is the first observed reading day in the current consent epoch.
use std::{error::Error, fmt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReadingReturnsError {
    Invalid,
    Unavailable,
}

impl fmt::Display for ReadingReturnsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl Error for ReadingReturnsError {}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadingReturnsReport {
    pub days: i32,
    pub measured_at_utc: String,
    pub observation_started_at_utc: Option<String>,
    pub last_observed_at_utc: Option<String>,
    pub collection_healthy: bool,
    pub current: ReadingReturnsPeriod,
    pub previous: ReadingReturnsPeriod,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadingReturnsPeriod {
    pub from_utc: String,
    pub until_utc: String,
    /// The selected cohort-enrollment period was observed continuously.
    /// Individual return horizons still have their own maturity/coverage gate.
    pub coverage_complete: bool,
    pub enrolled: i64,
    pub metrics: [ReadingReturnsMetric; 3],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadingReturnsMetric {
    pub day: i32,
    pub eligible: i64,
    pub returned: i64,
    pub pending: i64,
    pub unknown: i64,
}

pub(crate) fn validate_period(days: i32) -> Result<(), ReadingReturnsError> {
    match days {
        1 | 7 | 30 | 90 => Ok(()),
        _ => Err(ReadingReturnsError::Invalid),
    }
}
