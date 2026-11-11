//! Bounded observations of a particular book, not lifetime unique readers.
use super::returns::ReadingReturnsMetric;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookReadingReturnsReport {
    pub book_id: i64,
    pub book_title: String,
    pub days: i32,
    pub measured_at_utc: String,
    pub observation_started_at_utc: Option<String>,
    pub last_observed_at_utc: Option<String>,
    pub collection_healthy: bool,
    pub current: BookReadingReturnsPeriod,
    pub previous: BookReadingReturnsPeriod,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookReadingReturnsPeriod {
    pub from_utc: String,
    pub until_utc: String,
    pub coverage_complete: bool,
    /// One consented subject/book observation lasts through its D30 UTC day.
    /// After erasure a later accepted advance can start another observation.
    pub enrolled: i64,
    pub metrics: [ReadingReturnsMetric; 2],
}
