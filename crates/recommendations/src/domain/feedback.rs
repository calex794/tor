//! REC-13 pure validation. Absence of evidence is never a dislike event.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAX_ITEMS: usize = 20;
pub const TICKET_SECONDS: i64 = 900;
pub const OBSERVATION_SECONDS: i64 = 72 * 3600;
pub const ALGORITHM: &str = "recommendations_v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeedbackError {
    Invalid,
    Forbidden,
    NotFound,
    Conflict,
    IdempotencyConflict,
    Limited,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Surface {
    Home,
    BookDetail,
    ReaderEnd,
    Catalog,
    Library,
}
impl Surface {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::BookDetail => "book_detail",
            Self::ReaderEnd => "reader_end",
            Self::Catalog => "catalog",
            Self::Library => "library",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImpressionClaim {
    pub ticket: String,
    pub visible_fraction_per_mille: u16,
    pub continuous_visible_ms: u32,
    pub document_visible: bool,
}
impl ImpressionClaim {
    pub fn validate(&self) -> Result<(), FeedbackError> {
        if !valid_token(&self.ticket)
            || self.visible_fraction_per_mille > 1000
            || self.continuous_visible_ms > 60_000
        {
            return Err(FeedbackError::Invalid);
        }
        Ok(())
    }
    pub fn qualified(&self) -> bool {
        self.document_visible
            && self.visible_fraction_per_mille >= 500
            && self.continuous_visible_ms >= 1000
    }
}

#[derive(Clone, Copy, Deserialize, Serialize, Eq, PartialEq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    NotInterested,
    Clear,
}
impl Choice {
    pub fn hidden(self) -> bool {
        self == Self::NotInterested
    }
}

#[derive(Debug, Serialize, Eq, PartialEq)]
pub struct ConsentState {
    pub enabled: bool,
    pub revision: i64,
}
#[derive(Debug, Serialize, Eq, PartialEq)]
pub struct FeedbackReceipt {
    pub revision: i64,
    pub hidden: bool,
}
#[derive(Debug, Serialize)]
pub struct HiddenBook {
    pub book_id: String,
    pub title: String,
    pub revision: i64,
}
#[derive(Serialize)]
pub struct FeedItem {
    pub book_id: String,
    pub title: String,
    pub reason: String,
    pub source: String,
    pub algorithm_version: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ticket: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub feedback_revision: Option<i64>,
}
#[derive(Serialize)]
pub struct Feed {
    pub items: Vec<FeedItem>,
    pub generation_id: Option<String>,
    pub next_cursor: Option<String>,
    pub personalized: bool,
}
impl Feed {
    pub fn empty() -> Self {
        Self {
            items: Vec::new(),
            generation_id: None,
            next_cursor: None,
            personalized: false,
        }
    }
}
#[derive(Debug, Serialize)]
pub struct QualityRow {
    pub day: String,
    pub book_id: String,
    pub surface: String,
    pub source: String,
    pub reason: String,
    pub generation: String,
    pub algorithm_version: String,
    pub position: i16,
    pub impressions: i64,
    pub paired_clicks: i64,
    pub unpaired_clicks: i64,
    pub starts: i64,
    pub completions: i64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Observation {
    Pending,
    Positive,
    Censored,
    NoVerifiedStart,
}
pub fn observation(
    now: i64,
    clicked_at: Option<i64>,
    started: bool,
    readable: bool,
    telemetry_complete: bool,
) -> Observation {
    if started {
        return Observation::Positive;
    }
    let Some(clicked) = clicked_at else {
        return Observation::Censored;
    };
    if !readable || !telemetry_complete || now < clicked {
        return Observation::Censored;
    }
    if now.saturating_sub(clicked) < OBSERVATION_SECONDS {
        Observation::Pending
    } else {
        Observation::NoVerifiedStart
    }
}

pub fn valid_token(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}
pub fn token_digest(value: &str) -> Result<[u8; 32], FeedbackError> {
    if !valid_token(value) {
        return Err(FeedbackError::Invalid);
    }
    Ok(Sha256::digest(value.as_bytes()).into())
}
pub fn book_id(value: &str) -> Result<i64, FeedbackError> {
    let id = value.parse::<i64>().map_err(|_| FeedbackError::Invalid)?;
    if id <= 0 || id.to_string() != value {
        return Err(FeedbackError::Invalid);
    }
    Ok(id)
}
pub fn rollout_allowed(subject: &str, percent: i16) -> bool {
    if ![0, 1, 5, 10, 25, 50, 100].contains(&percent) {
        return false;
    }
    let digest = Sha256::digest(format!("rec13-rollout-v1:{subject}").as_bytes());
    let bucket = u16::from_be_bytes([digest[0], digest[1]]) % 100;
    bucket < percent as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn absence_never_becomes_dislike() {
        assert_eq!(
            observation(1, None, false, true, true),
            Observation::Censored
        );
        assert_eq!(
            observation(100, Some(1), false, true, true),
            Observation::Pending
        );
        assert_eq!(
            observation(400_000, Some(1), false, true, false),
            Observation::Censored
        );
        assert_eq!(
            observation(400_000, Some(1), false, false, true),
            Observation::Censored
        );
        assert_eq!(
            observation(400_000, Some(1), false, true, true),
            Observation::NoVerifiedStart
        );
        assert_eq!(
            observation(400_000, Some(1), true, false, false),
            Observation::Positive
        );
    }
    #[test]
    fn limits_and_canonical_ids() {
        for value in ["0", "-1", "01", "+1", " 1", "9223372036854775808"] {
            assert_eq!(book_id(value), Err(FeedbackError::Invalid));
        }
        assert_eq!(book_id("123"), Ok(123));
        assert!(!valid_token(&"x".repeat(44)));
        assert!(valid_token(&"x".repeat(43)));
    }
    #[test]
    fn view_is_a_bounded_client_claim() {
        let mut claim = ImpressionClaim {
            ticket: "x".repeat(43),
            visible_fraction_per_mille: 500,
            continuous_visible_ms: 1000,
            document_visible: true,
        };
        assert!(claim.validate().is_ok() && claim.qualified());
        claim.document_visible = false;
        assert!(!claim.qualified());
        claim.visible_fraction_per_mille = 1001;
        assert_eq!(claim.validate(), Err(FeedbackError::Invalid));
    }
    #[test]
    fn deterministic_rollout_is_off_or_stable() {
        assert!(!rollout_allowed("reader", 0));
        assert!(rollout_allowed("reader", 100));
        assert!(!rollout_allowed("reader", 99));
        assert_eq!(rollout_allowed("reader", 25), rollout_allowed("reader", 25));
    }
}
