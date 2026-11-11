//! Completed UTC-hour windows over retained C business facts, never browser visits.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ClosedReadingCounts {
    /// First verified start per account/chapter, including authorized private reading.
    pub starts: i64,
    /// First retained verified chapter completion per account/book in the current
    /// public catalogue. Later chapters are not another completed pair.
    pub completed_book_pairs: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosedReadingReport {
    pub days: i32,
    pub measured_at_utc: String,
    /// Exclusive UTC-hour boundary shared by both adjacent equal windows.
    pub until_epoch: i64,
    pub current: ClosedReadingCounts,
    pub previous: ClosedReadingCounts,
}
