//! Provider-reported search aggregates, never visits, identities or reading attribution.
use super::seo_urls::TrustedSeoOrigin;
use serde::{Deserialize, Serialize};

pub const MAX_PAGES: usize = 50_000;
pub const MAX_COUNT: u64 = 1_000_000_000_000;
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Google,
    Yandex,
}
impl Provider {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "google" => Some(Self::Google),
            "yandex" => Some(Self::Yandex),
            _ => None,
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Google => "google",
            Self::Yandex => "yandex",
        }
    }
    pub fn calendar(self) -> &'static str {
        match self {
            Self::Google => "America/Los_Angeles",
            Self::Yandex => "provider_date",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SeoAnalyticsError {
    Invalid,
    Unavailable,
    Unauthorized,
    RateLimited,
    Stale,
    InvalidResponse,
}
impl SeoAnalyticsError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::Unavailable => "unavailable",
            Self::Unauthorized => "authorization",
            Self::RateLimited => "rate_limited",
            Self::Stale => "stale",
            Self::InvalidResponse => "invalid_response",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    pub clicks: u64,
    pub impressions: u64,
    pub position: Option<f64>,
}
impl Metrics {
    pub fn valid(&self) -> bool {
        self.clicks <= self.impressions
            && self.impressions <= MAX_COUNT
            && self
                .position
                .is_none_or(|p| p.is_finite() && (0.0..=1e9).contains(&p))
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageMetrics {
    pub path: String,
    pub metrics: Metrics,
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub totals: Option<Metrics>,
    pub pages: Vec<PageMetrics>,
    pub provider_rows: u64,
    pub omitted_rows: u64,
    pub limit_reached: bool,
}
impl Snapshot {
    pub fn validate(&self, origin: &str) -> Result<(), SeoAnalyticsError> {
        if self.pages.len() > MAX_PAGES
            || self.totals.as_ref().is_some_and(|m| !m.valid())
            || self.provider_rows > MAX_PAGES as u64
            || self.pages.len() as u64 + self.omitted_rows > self.provider_rows
            || self.omitted_rows > self.provider_rows
        {
            return Err(SeoAnalyticsError::InvalidResponse);
        }
        let mut seen = std::collections::BTreeSet::new();
        for page in &self.pages {
            if !page.metrics.valid()
                || canonical_path(origin, &format!("{origin}{}", page.path)).as_deref()
                    != Some(page.path.as_str())
                || !seen.insert(&page.path)
            {
                return Err(SeoAnalyticsError::InvalidResponse);
            }
        }
        Ok(())
    }
    pub fn available(&self) -> bool {
        self.totals.is_some() || !self.pages.is_empty()
    }
}
#[derive(Clone, Debug)]
pub struct SourceKey {
    pub provider: Provider,
    pub property: String,
    pub origin: String,
}
impl SourceKey {
    pub fn validate(&self) -> Result<(), SeoAnalyticsError> {
        if TrustedSeoOrigin::from_config(&self.origin).is_none() || self.origin.ends_with('/') {
            return Err(SeoAnalyticsError::Invalid);
        }
        let host = &self.origin[8..];
        let valid = match self.provider {
            Provider::Google => {
                self.property == format!("{}/", self.origin)
                    || self.property == format!("sc-domain:{host}")
            }
            // Store the explicit verified host identifier, not a caller-controlled URL.
            Provider::Yandex => self.property == format!("https:{host}:443"),
        };
        if !valid {
            return Err(SeoAnalyticsError::Invalid);
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct Source {
    pub id: i64,
    pub key: SourceKey,
}
#[derive(Clone, Debug)]
pub struct Job {
    pub id: i64,
    pub epoch: i64,
    pub source: Source,
    pub day: String,
}
#[derive(Clone, Debug)]
pub struct SourceStatus {
    pub source: Source,
    pub last_success: Option<String>,
    pub last_error: Option<String>,
    pub pending: i64,
    pub running: i64,
}
#[derive(Clone, Debug)]
pub struct PeriodTotals {
    pub from: String,
    pub until: String,
    pub measured_days: i64,
    pub total_days: i64,
    pub clicks: Option<i64>,
    pub impressions: Option<i64>,
    pub position: Option<f64>,
    pub limited_days: i64,
    pub omitted_rows: i64,
}
#[derive(Clone, Debug)]
pub struct ReportPage {
    pub path: String,
    pub title: String,
    pub clicks: i64,
    pub impressions: i64,
    pub position: Option<f64>,
    pub measured_days: i64,
}
#[derive(Clone, Debug)]
pub struct SeoReport {
    pub days: i32,
    pub provider: Provider,
    pub sources: Vec<SourceStatus>,
    pub current: PeriodTotals,
    pub previous: PeriodTotals,
    pub pages: Vec<ReportPage>,
    pub next: Option<String>,
    pub measured_at: String,
}
/// Accept only canonical, same-origin public route shapes. Queries, fragments,
/// credentials, alternate hosts and private routes never enter storage.
pub fn canonical_path(origin: &str, url: &str) -> Option<String> {
    if TrustedSeoOrigin::from_config(origin).is_none() || origin.ends_with('/') {
        return None;
    }
    let path = url.strip_prefix(origin)?;
    if matches!(path, "/" | "/catalog") {
        return Some(path.into());
    }
    let tail = path
        .strip_prefix("/books/")
        .or_else(|| path.strip_prefix("/reader/"))?;
    if tail.is_empty() || tail.starts_with('0') || !tail.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let n: i64 = tail.parse().ok()?;
    (n > 0).then(|| path.to_owned())
}
pub fn valid_day(day: &str) -> bool {
    let b = day.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || !b
            .iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
    {
        return false;
    }
    let year: u32 = day[..4].parse().unwrap_or(0);
    let month: usize = day[5..7].parse().unwrap_or(0);
    let date: u32 = day[8..].parse().unwrap_or(0);
    if !(2000..=2100).contains(&year) || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    date > 0 && date <= days[month - 1]
}
