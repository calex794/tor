//! Acknowledged HTML-worker intervals; neither visitors nor installation coverage.
pub const HTML_HEALTH_LEASE_MS: i64 = 30_000;
pub const HTML_HEALTH_GAP_MS: i64 = 120_000;
pub const HTML_CLOCK_MARGIN_MS: i64 = 2_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtmlHealthError {
    Invalid,
    Unavailable,
    Expired,
}

#[derive(Clone, Copy, Debug)]
pub struct HtmlCheckpoint {
    pub epoch: i64,
    pub generation: i64,
    pub cut_ms: i64,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct HtmlHealthPeriod {
    pub confirmed_hours: i32,
    pub loss_hours: i32,
    pub error_hours: i32,
}
impl HtmlHealthPeriod {
    pub fn has_failure(self) -> bool {
        self.loss_hours > 0 || self.error_hours > 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HtmlHealthReport {
    pub until_epoch: i64,
    pub current: HtmlHealthPeriod,
    pub previous: HtmlHealthPeriod,
}
