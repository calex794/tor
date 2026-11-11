//! Snapshot of explicit product choices, not a history of tracking events.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct IntentCounts {
    pub library_links: i64,
    pub library_accounts: i64,
    pub follow_links: i64,
    pub follow_accounts: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntentReport {
    pub days: i32,
    pub measured_at_utc: String,
    pub current_from_utc: String,
    pub previous_from_utc: String,
    /// All links that exist in this one database snapshot, not historical totals.
    pub totals: IntentCounts,
    /// Surviving links created within [current_from, measured_at).
    pub current: IntentCounts,
    /// Surviving links created within [previous_from, current_from).
    pub previous: IntentCounts,
}
