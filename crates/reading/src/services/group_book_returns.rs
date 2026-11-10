//! Same-book D7/D30 observations for a current public genre or tag group.
use super::{
    book_returns::BookReadingReturnsPeriod,
    returns::{validate_period, ReadingReturnsError},
};
use tor_contracts::audience::normalize_audience_label;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupBookReadingReturnsKind {
    Genres,
    Tags,
}

impl GroupBookReadingReturnsKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Genres => "genres",
            Self::Tags => "tags",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupBookReadingReturnsQuery {
    pub days: i32,
    pub kind: GroupBookReadingReturnsKind,
    /// None selects books without this taxonomy, never all books.
    pub label: Option<String>,
}

impl GroupBookReadingReturnsQuery {
    pub fn new(
        days: i32,
        kind: GroupBookReadingReturnsKind,
        label: Option<String>,
    ) -> Result<Self, ReadingReturnsError> {
        validate_period(days)?;
        let provided = label.is_some();
        let label = normalize_audience_label(label).map_err(|_| ReadingReturnsError::Invalid)?;
        if provided && label.is_none() {
            return Err(ReadingReturnsError::Invalid);
        }
        Ok(Self { days, kind, label })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupBookReadingReturnsReport {
    pub query: GroupBookReadingReturnsQuery,
    /// Unique currently eligible books, including those without observations.
    pub matched_books: i64,
    pub measured_at_utc: String,
    pub observation_started_at_utc: Option<String>,
    pub last_observed_at_utc: Option<String>,
    pub collection_healthy: bool,
    /// Counts sum bounded subject/book observations, not unique readers.
    pub current: BookReadingReturnsPeriod,
    pub previous: BookReadingReturnsPeriod,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exactly_one_group_uses_shared_normalization_and_periods() {
        for days in [1, 7, 30, 90] {
            let query = GroupBookReadingReturnsQuery::new(
                days,
                GroupBookReadingReturnsKind::Genres,
                Some("  МАГИЯ\u{a0}  И Мир  ".into()),
            )
            .unwrap();
            assert_eq!(query.label.as_deref(), Some("магия и мир"));
            assert_eq!(query.kind.as_str(), "genres");
            assert!(GroupBookReadingReturnsQuery::new(
                days,
                GroupBookReadingReturnsKind::Tags,
                None
            )
            .unwrap()
            .label
            .is_none());
        }
        for days in [0, 2, 8, 31, 91, i32::MAX] {
            assert_eq!(
                GroupBookReadingReturnsQuery::new(days, GroupBookReadingReturnsKind::Genres, None),
                Err(ReadingReturnsError::Invalid)
            );
        }
        for label in [
            "".into(),
            "   ".into(),
            "x\ny".into(),
            "я".repeat(41),
            " ".repeat(513),
        ] {
            assert_eq!(
                GroupBookReadingReturnsQuery::new(
                    90,
                    GroupBookReadingReturnsKind::Tags,
                    Some(label)
                ),
                Err(ReadingReturnsError::Invalid)
            );
        }
    }
}
