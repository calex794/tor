//! Current public-book metadata over retained anonymous discovery facts.
use super::{validate_period, DiscoveryError, DiscoveryHealth};

pub const BOOK_DISCOVERY_PAGE_SIZE: usize = 20;
pub const BOOK_DISCOVERY_FILTER_OPTIONS: usize = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookDiscoveryQuery {
    pub days: i32,
    pub genre: Option<String>,
    pub tag: Option<String>,
    pub without_genre: bool,
    pub without_tag: bool,
    /// Ascending public book ID; independent of changing counters and titles.
    pub after_id: Option<i64>,
}

impl BookDiscoveryQuery {
    pub fn new(
        days: i32,
        genre: Option<String>,
        tag: Option<String>,
        after_id: Option<i64>,
    ) -> Result<Self, DiscoveryError> {
        Self::with_missing(days, genre, tag, false, false, after_id)
    }

    pub fn with_missing(
        days: i32,
        genre: Option<String>,
        tag: Option<String>,
        without_genre: bool,
        without_tag: bool,
        after_id: Option<i64>,
    ) -> Result<Self, DiscoveryError> {
        validate_period(days)?;
        if after_id.is_some_and(|id| id <= 0) {
            return Err(DiscoveryError::Invalid);
        }
        let genre = normalize_label(genre)?;
        let tag = normalize_label(tag)?;
        if (without_genre && genre.is_some()) || (without_tag && tag.is_some()) {
            return Err(DiscoveryError::Invalid);
        }
        Ok(Self {
            days,
            genre,
            tag,
            without_genre,
            without_tag,
            after_id,
        })
    }

    pub fn validate(&self) -> Result<(), DiscoveryError> {
        Self::with_missing(
            self.days,
            self.genre.clone(),
            self.tag.clone(),
            self.without_genre,
            self.without_tag,
            self.after_id,
        )
        .map(|_| ())
    }
}

pub(super) fn normalize_label(value: Option<String>) -> Result<Option<String>, DiscoveryError> {
    tor_contracts::audience::normalize_audience_label(value).map_err(|_| DiscoveryError::Invalid)
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BookDiscoveryMetrics {
    /// Catalog/search links issued in responses, not observed impressions.
    pub issued: Option<i64>,
    /// Client-observed recommendation impressions only.
    pub impressions: Option<i64>,
    /// Navigation facts from catalog, search and recommendations.
    pub clicks: Option<i64>,
    /// None also covers any included historical REC bucket without this field.
    pub book_opens: Option<i64>,
    /// Consented first-book starts, including those without a known source.
    pub starts: Option<i64>,
}

impl BookDiscoveryMetrics {
    pub fn observation(&self) -> BookDiscoveryObservation {
        if self.starts.is_some_and(|value| value > 0) {
            BookDiscoveryObservation::WithStarts
        } else if [self.issued, self.impressions, self.clicks, self.book_opens]
            .into_iter()
            .flatten()
            .any(|value| value > 0)
        {
            BookDiscoveryObservation::ObservedWithoutStarts
        } else {
            BookDiscoveryObservation::NoObservations
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BookDiscoveryObservation {
    NoObservations,
    ObservedWithoutStarts,
    WithStarts,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookDiscoveryCounts {
    pub no_observations: i64,
    pub observed_without_starts: i64,
    pub with_starts: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookDiscoveryPeriod {
    pub from_day: String,
    pub until_day_exclusive: String,
    pub includes_partial_today: bool,
    /// All matching currently public books, before pagination.
    pub totals: BookDiscoveryMetrics,
    pub counts: BookDiscoveryCounts,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookDiscoveryRow {
    pub book_id: i64,
    pub title: String,
    /// Present metadata, not the historical metadata at event time.
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub current: BookDiscoveryMetrics,
    pub previous: BookDiscoveryMetrics,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookDiscoveryReport {
    pub days: i32,
    pub measured_at_utc: String,
    pub current: BookDiscoveryPeriod,
    pub previous: BookDiscoveryPeriod,
    pub health: DiscoveryHealth,
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    /// The first 100 suggestions are only an input aid; other labels remain valid.
    pub filter_options_truncated: bool,
    pub rows: Vec<BookDiscoveryRow>,
    pub next_after_id: Option<i64>,
    pub matched_books: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_share_current_metadata_spelling_and_are_bounded() {
        let query = BookDiscoveryQuery::new(
            90,
            Some("  Фэнтези  ".into()),
            Some("  Два   мира ".into()),
            Some(7),
        )
        .unwrap();
        assert_eq!(query.genre.as_deref(), Some("фэнтези"));
        assert_eq!(query.tag.as_deref(), Some("два мира"));
        assert_eq!(
            BookDiscoveryQuery::new(1, Some("  ".into()), None, None)
                .unwrap()
                .genre,
            None
        );
        for value in [
            "a".repeat(41),
            " ".repeat(513),
            "tag\nname".into(),
            "tag\0".into(),
        ] {
            assert_eq!(
                BookDiscoveryQuery::new(1, None, Some(value), None),
                Err(DiscoveryError::Invalid)
            );
        }
        for id in [0, -1, i64::MIN] {
            assert_eq!(
                BookDiscoveryQuery::new(1, None, None, Some(id)),
                Err(DiscoveryError::Invalid)
            );
        }
        assert_eq!(
            BookDiscoveryQuery::new(2, None, None, None),
            Err(DiscoveryError::Invalid)
        );
    }

    #[test]
    fn observations_never_call_unknown_source_reading_unread() {
        let mut value = BookDiscoveryMetrics::default();
        assert_eq!(
            value.observation(),
            BookDiscoveryObservation::NoObservations
        );
        value.issued = Some(0);
        assert_eq!(
            value.observation(),
            BookDiscoveryObservation::NoObservations
        );
        value.clicks = Some(1);
        assert_eq!(
            value.observation(),
            BookDiscoveryObservation::ObservedWithoutStarts
        );
        value.starts = Some(1);
        assert_eq!(value.observation(), BookDiscoveryObservation::WithStarts);
    }

    #[test]
    fn missing_label_filters_are_explicit_and_reject_conflicts() {
        let missing = BookDiscoveryQuery::with_missing(7, None, None, true, true, None).unwrap();
        assert!(missing.without_genre && missing.without_tag);
        for (genre, tag, no_genre, no_tag) in [
            (Some("жанр".into()), None, true, false),
            (None, Some("тег".into()), false, true),
        ] {
            assert_eq!(
                BookDiscoveryQuery::with_missing(7, genre, tag, no_genre, no_tag, None),
                Err(DiscoveryError::Invalid)
            );
        }
        let mut forged = missing;
        forged.genre = Some("жанр".into());
        assert_eq!(forged.validate(), Err(DiscoveryError::Invalid));
    }
}
