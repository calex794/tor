//! Numeric book-visit cohorts; C alone certifies reading and saved actions.
//! Saves and follows are parallel outcomes, not steps after completion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlowFunnelError {
    Invalid,
    Unavailable,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FlowFunnelCounts {
    pub cohorts: i64,
    pub detail_opens: i64,
    pub reader_opens: i64,
    pub starts: i64,
    pub completions: i64,
    pub saves: i64,
    pub follows: i64,
    pub new_readers: i64,
    pub d7_anchors: i64,
    pub d7_pending: i64,
    pub d7_returned: i64,
    pub d7_unknown: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowFunnelRow {
    pub acquisition_hint: i16,
    /// 0 recommendation, 1 catalogue, 2 internal search, 3 unknown.
    pub discovery_kind: i16,
    pub counts: FlowFunnelCounts,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowFunnelPeriod {
    pub from_utc: String,
    pub until_utc: String,
    pub counts: Option<FlowFunnelCounts>,
    /// Only the complete retained 30-calendar-day observation window supports
    /// these diagnostics. They are not extrapolated from archived daily sums.
    pub pending_book_observations: Option<i64>,
    pub unmatched_book_observations: Option<i64>,
    pub rows: Vec<FlowFunnelRow>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlowFunnelReport {
    pub days: i32,
    pub measured_at_utc: String,
    pub current: FlowFunnelPeriod,
    pub previous: FlowFunnelPeriod,
    pub worker_last_success_utc: Option<String>,
    pub worker_collecting: bool,
    pub worker_backlog: bool,
}
