use sha2::{Digest, Sha256};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalError {
    Invalid,
    Unavailable,
    Stale,
}

macro_rules! codes {
    ($name:ident { $($variant:ident => $code:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name { $($variant),+ }
        impl $name {
            pub const fn code(self) -> &'static str {
                match self { $(Self::$variant => $code),+ }
            }
        }
        impl FromStr for $name {
            type Err = SignalError;
            fn from_str(value: &str) -> Result<Self, Self::Err> {
                match value { $($code => Ok(Self::$variant),)+ _ => Err(SignalError::Invalid) }
            }
        }
    };
}

codes!(Rule {
    HtmlQuality => "html_quality",
    HtmlChange => "html_change",
    ReadingStarts => "reading_starts",
    ReadingCompletions => "reading_completions",
    GoogleQuality => "google_quality",
    GoogleClicks => "google_clicks",
    YandexQuality => "yandex_quality",
    YandexClicks => "yandex_clicks",
    ExposureQuality => "exposure_quality",
    ExposureChange => "exposure_change",
    UnderexposedBooks => "underexposed_books",
    UnderexposedGenres => "underexposed_genres",
    PublicPageLoss => "public_page_loss",
});
impl Rule {
    pub const ALL: [Self; 13] = [
        Self::HtmlQuality, Self::HtmlChange, Self::ReadingStarts,
        Self::ReadingCompletions, Self::GoogleQuality, Self::GoogleClicks,
        Self::YandexQuality, Self::YandexClicks, Self::ExposureQuality,
        Self::ExposureChange, Self::UnderexposedBooks, Self::UnderexposedGenres,
        Self::PublicPageLoss,
    ];
    pub fn accepts_days(self, days: i32) -> bool {
        if self == Self::PublicPageLoss { days == 0 } else { [1, 7, 30, 90].contains(&days) }
    }
}
codes!(Status {
    Unknown => "unknown", Active => "active", Healthy => "healthy", Disabled => "disabled",
});
codes!(Reason {
    WithinThreshold => "within_threshold",
    RecordedDrop => "recorded_drop",
    RecordedGrowth => "recorded_growth",
    MissingMeasurement => "missing_measurement",
    PartialCoverage => "partial_coverage",
    SourceFailure => "source_failure",
    DiscardedObservations => "discarded_observations",
    LimitedPages => "limited_pages",
    SmallBaseline => "small_baseline",
    SourceUnavailable => "source_unavailable",
    SourceDisabled => "source_disabled",
    LowExposure => "low_exposure",
    ObserverCoverage => "observer_coverage",
    PublicPageLoss => "public_page_loss",
});

/// Counts and coverage are different facts. None never denotes a measured zero.
/// units are complete hours/days (latest public-page sample: checked targets).
/// sample is rendered cards for exposure; ack is per thousand registered proofs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Evidence {
    pub enabled: bool,
    pub available: bool,
    pub current: Option<i64>,
    pub previous: Option<i64>,
    pub current_units: i32,
    pub previous_units: i32,
    pub expected_units: i32,
    pub known_discards: i64,
    pub source_failed: bool,
    pub limited_units: i32,
    pub omitted_rows: i64,
    pub current_ack_per_mille: Option<i32>,
    pub previous_ack_per_mille: Option<i32>,
    pub current_sample: i64,
    pub previous_sample: i64,
    pub unknown_entities: i64,
}
impl Evidence {
    pub fn missing(enabled: bool) -> Self {
        Self {
            enabled, available: false, current: None, previous: None,
            current_units: 0, previous_units: 0, expected_units: 0,
            known_discards: 0, source_failed: false, limited_units: 0,
            omitted_rows: 0, current_ack_per_mille: None,
            previous_ack_per_mille: None, current_sample: 0, previous_sample: 0,
            unknown_entities: 0,
        }
    }
    pub fn validate(self) -> Result<Self, SignalError> {
        if self.current.is_some_and(|n| n < 0)
            || self.previous.is_some_and(|n| n < 0)
            || [self.current_units, self.previous_units, self.expected_units, self.limited_units]
                .iter().any(|n| *n < 0)
            || [self.known_discards, self.omitted_rows, self.current_sample,
                self.previous_sample, self.unknown_entities].iter().any(|n| *n < 0)
            || [self.current_ack_per_mille, self.previous_ack_per_mille]
                .into_iter().flatten().any(|n| !(0..=1000).contains(&n))
        {
            Err(SignalError::Invalid)
        } else {
            Ok(self)
        }
    }
    pub fn current_complete(self) -> bool {
        self.expected_units > 0 && self.current_units == self.expected_units
            && self.known_discards == 0 && !self.source_failed
    }
    pub fn comparable(self) -> bool {
        self.current_complete() && self.previous_units == self.expected_units
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Evaluation {
    pub status: Status,
    pub reason: Reason,
}
impl Evaluation {
    pub const fn new(status: Status, reason: Reason) -> Self {
        Self { status, reason }
    }
}

/// Store only this digest. Parts must describe a source/configuration, never a reader.
pub fn source_scope(parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"flow-signals-v2");
    for part in parts {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    format!("{:x}", hash.finalize())
}
pub fn valid_scope(scope: &str) -> bool {
    scope.len() == 64 && scope.bytes().all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalWindow {
    /// Actual owner boundary (UTC seconds), never recomputed when rendering.
    pub until_epoch: Option<i64>,
    /// Persisted source-native dates/zone or explanation for an unavailable source.
    pub label: String,
}
impl SignalWindow {
    pub fn validate(&self) -> Result<(), SignalError> {
        if self.until_epoch.is_some_and(|n| n < 0)
            || self.label.len() > 240 || self.label.chars().any(char::is_control)
        {
            Err(SignalError::Invalid)
        } else {
            Ok(())
        }
    }
}
