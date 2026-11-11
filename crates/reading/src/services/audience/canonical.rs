//! Current public-catalog counts from existing C facts, never a new event stream.
use super::{validate_period, AudienceError};
use tor_contracts::audience::normalize_audience_label;

pub const CANONICAL_PAGE_SIZE: usize = 20;
pub const COMPARISON_MIN_PAIRS: i64 = 10;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CanonicalDimension {
    #[default]
    Books,
    Genres,
    Tags,
}
impl CanonicalDimension {
    pub fn parse(value: &str) -> Result<Self, AudienceError> {
        match value {
            "books" => Ok(Self::Books),
            "genres" => Ok(Self::Genres),
            "tags" => Ok(Self::Tags),
            _ => Err(AudienceError::Invalid),
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            Self::Books => "books",
            Self::Genres => "genres",
            Self::Tags => "tags",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalQuery {
    pub days: i32,
    pub dimension: CanonicalDimension,
    pub genre: Option<String>,
    pub tag: Option<String>,
    pub without_genre: bool,
    pub without_tag: bool,
    pub after_id: Option<i64>,
    pub after_label: Option<String>,
}
impl CanonicalQuery {
    pub fn new(days: i32, dimension: CanonicalDimension) -> Self {
        Self {
            days,
            dimension,
            genre: None,
            tag: None,
            without_genre: false,
            without_tag: false,
            after_id: None,
            after_label: None,
        }
    }
    pub fn validated(mut self) -> Result<Self, AudienceError> {
        validate_period(self.days)?;
        self.genre = normalize_audience_label(self.genre).map_err(|_| AudienceError::Invalid)?;
        self.tag = normalize_audience_label(self.tag).map_err(|_| AudienceError::Invalid)?;
        if self.after_id.is_some_and(|id| id <= 0)
            || (self.without_genre && self.genre.is_some())
            || (self.without_tag && self.tag.is_some())
            || (self.dimension == CanonicalDimension::Books && self.after_label.is_some())
            || (self.dimension != CanonicalDimension::Books && self.after_id.is_some())
        {
            return Err(AudienceError::Invalid);
        }
        if let Some(value) = &self.after_label {
            // A cursor is the exact retained label, not a newly normalized filter.
            if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
                return Err(AudienceError::Invalid);
            }
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CanonicalCounts {
    pub books: i64,
    /// First retained verified chapter completion per account/book, in each window.
    pub current_pairs: i64,
    pub previous_pairs: i64,
    /// Current surviving product relationships; these are not historical actions.
    pub library_links: i64,
    pub follow_links: i64,
}
impl CanonicalCounts {
    pub fn comparison_delta(self) -> Option<i64> {
        (self.current_pairs >= COMPARISON_MIN_PAIRS && self.previous_pairs >= COMPARISON_MIN_PAIRS)
            .then(|| self.current_pairs - self.previous_pairs)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalRow {
    pub book_id: Option<i64>,
    /// None is the real group with no label. Never a string sentinel.
    pub label: Option<String>,
    pub counts: CanonicalCounts,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalReport {
    pub measured_at_utc: String,
    pub current_from_utc: String,
    pub previous_from_utc: String,
    /// Across all eligible books before expansion/pagination; overlapping groups do not multiply it.
    pub totals: CanonicalCounts,
    pub rows: Vec<CanonicalRow>,
    pub next_after_id: Option<i64>,
    pub next_after_label: Option<String>,
}

/// A bounded descriptive ranking of retained first account/book completions.
/// The threshold is a presentation caveat, not statistical significance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalRankRow {
    pub book_id: Option<i64>,
    pub label: Option<String>,
    pub counts: CanonicalCounts,
    pub small_current_sample: bool,
    pub small_previous_sample: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalRankReport {
    pub measured_at_utc: String,
    pub current_from_utc: String,
    pub previous_from_utc: String,
    /// Whole eligible scope, before group expansion and top-20 selection.
    pub totals: CanonicalCounts,
    pub rows: Vec<CanonicalRankRow>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_cursors_filters_periods_and_small_samples_are_bounded() {
        for days in [1, 7, 30, 90] {
            for dimension in [
                CanonicalDimension::Books,
                CanonicalDimension::Genres,
                CanonicalDimension::Tags,
            ] {
                assert!(CanonicalQuery::new(days, dimension).validated().is_ok());
            }
        }
        for days in [-1, 0, 2, 91] {
            assert!(CanonicalQuery::new(days, CanonicalDimension::Books)
                .validated()
                .is_err());
        }
        let mut query = CanonicalQuery::new(7, CanonicalDimension::Books);
        query.after_label = Some("fiction".into());
        assert!(query.validated().is_err());
        let mut query = CanonicalQuery::new(7, CanonicalDimension::Tags);
        query.after_label = Some("  Exact Label  ".into());
        assert_eq!(
            query.clone().validated().unwrap().after_label,
            query.after_label
        );
        query.after_label = Some("bad\nlabel".into());
        assert!(query.validated().is_err());
        let mut query = CanonicalQuery::new(7, CanonicalDimension::Books);
        query.genre = Some(" FANTASY ".into());
        query.without_genre = true;
        assert!(query.validated().is_err());
        assert_eq!(
            CanonicalCounts {
                current_pairs: 9,
                previous_pairs: 20,
                ..Default::default()
            }
            .comparison_delta(),
            None
        );
        assert_eq!(
            CanonicalCounts {
                current_pairs: 11,
                previous_pairs: 20,
                ..Default::default()
            }
            .comparison_delta(),
            Some(-9)
        );
    }
}
