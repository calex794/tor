//! A persisted card opportunity and browser observations, never people or billable ads.
use serde::{Deserialize, Serialize};
use tor_contracts::exposure::{CardObservation, ExposureSurface};

pub const PROOF_TTL_SECONDS: i64 = 900;
pub const MAX_PENDING_PROOFS: i64 = 100_000;
pub const MIN_OBSERVED_SAMPLE: i64 = 20;
pub const MIN_OBSERVER_COVERAGE_PERCENT: i64 = 80;
pub const LOW_CONFIRMED_VIEWS: i64 = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExposureError {
    Invalid,
    Missing,
    Expired,
    TooEarly,
    Forbidden,
    Capacity,
    Unavailable,
}
#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    Rendered,
    View,
    Click,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservationEvent {
    pub proof: String,
    pub kind: EventKind,
    #[serde(default)]
    pub visible_percent: u16,
    #[serde(default)]
    pub continuous_visible_ms: u32,
    #[serde(default)]
    pub foreground: bool,
}
impl ObservationEvent {
    pub fn validate(&self) -> Result<(), ExposureError> {
        use tor_contracts::exposure::{VIEWABLE_DURATION_MS, VIEWABLE_PERCENT};
        super::feedback::valid_token(&self.proof)
            .then_some(())
            .ok_or(ExposureError::Invalid)?;
        if self.visible_percent > 100 || self.continuous_visible_ms > 900_000 {
            return Err(ExposureError::Invalid);
        }
        if self.kind == EventKind::View
            && (!self.foreground
                || self.visible_percent < VIEWABLE_PERCENT
                || u64::from(self.continuous_visible_ms) < VIEWABLE_DURATION_MS)
        {
            return Err(ExposureError::Invalid);
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct IssuedObservation {
    pub digest: [u8; 32],
    pub card: CardObservation,
}
#[derive(Clone, Debug)]
pub struct ProofContext {
    pub card: CardObservation,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObservationReceipt {
    Recorded,
    Replay,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub opportunities: i64,
    pub rendered: i64,
    pub views: i64,
    pub clicks: i64,
    pub expired_unobserved: i64,
    pub replayed: i64,
}
impl Counts {
    pub fn classification(self) -> ObservationClass {
        if self.rendered < MIN_OBSERVED_SAMPLE
            || self.opportunities == 0
            || i128::from(self.rendered) * 100
                < i128::from(self.opportunities) * i128::from(MIN_OBSERVER_COVERAGE_PERCENT)
        {
            ObservationClass::Unknown
        } else if self.views < LOW_CONFIRMED_VIEWS {
            ObservationClass::LowObserved
        } else {
            ObservationClass::Observed
        }
    }
    pub fn known_views(self) -> Option<i64> {
        (self.rendered > 0).then_some(self.views)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObservationClass {
    Unknown,
    LowObserved,
    Observed,
}
impl ObservationClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::LowObserved => "low",
            Self::Observed => "observed",
        }
    }
}
#[derive(Clone, Debug)]
pub struct ExposureFilter {
    pub days: i32,
    pub surface: Option<ExposureSurface>,
    pub after: i64,
    pub class: Option<ObservationClass>,
    pub group: Option<(String, String)>,
    pub campaign: Option<i64>,
    pub editorial: Option<bool>,
}
impl ExposureFilter {
    pub fn validate(&self) -> Result<(), ExposureError> {
        if !matches!(self.days, 1 | 7 | 30 | 90)
            || self.after < 0
            || self.campaign.is_some_and(|id| id <= 0)
            || self.campaign.is_some() && self.editorial == Some(false)
            || self.group.as_ref().is_some_and(|(kind, label)| {
                !matches!(kind.as_str(), "genre" | "tag")
                    || label.len() > 160
                    || label.chars().any(char::is_control)
            })
        {
            return Err(ExposureError::Invalid);
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct ExposureBook {
    pub id: i64,
    pub title: String,
    pub counts: Counts,
    pub previous: Counts,
}
#[derive(Clone, Debug)]
pub struct ExposureGroup {
    pub kind: String,
    pub label: String,
    pub books: i64,
    pub counts: Counts,
}
#[derive(Clone, Debug)]
pub struct ExposureReport {
    pub checked_at_epoch: i64,
    pub books: i64,
    pub current: Counts,
    pub previous: Counts,
    pub low_observed_books: i64,
    pub unknown_books: i64,
    pub rows: Vec<ExposureBook>,
    pub next: Option<i64>,
    pub groups: Vec<ExposureGroup>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_signals_never_classify_as_unseen_or_unique() {
        assert_eq!(Counts::default().known_views(), None);
        assert_eq!(
            Counts {
                opportunities: 100,
                ..Counts::default()
            }
            .classification(),
            ObservationClass::Unknown
        );
        assert_eq!(
            Counts {
                opportunities: 100,
                rendered: 20,
                ..Counts::default()
            }
            .classification(),
            ObservationClass::Unknown
        );
        assert_eq!(
            Counts {
                opportunities: 25,
                rendered: 20,
                ..Counts::default()
            }
            .classification(),
            ObservationClass::LowObserved
        );
        assert_eq!(
            Counts {
                opportunities: 25,
                rendered: 20,
                views: 5,
                ..Counts::default()
            }
            .classification(),
            ObservationClass::Observed
        );
    }
}
