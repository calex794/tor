//! Current genres/tags over anonymous book totals; groups may overlap.
use super::{
    books::{normalize_label, BookDiscoveryCounts, BookDiscoveryMetrics, BookDiscoveryPeriod},
    validate_period, DiscoveryError, DiscoveryHealth,
};

pub const GROUP_DISCOVERY_PAGE_SIZE: usize = 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GroupDiscoveryKind {
    Genres,
    Tags,
}

impl GroupDiscoveryKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Genres => "genres",
            Self::Tags => "tags",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupDiscoveryQuery {
    pub days: i32,
    pub kind: GroupDiscoveryKind,
    /// Strictly after a canonical label in byte order. The missing group is first.
    pub after: Option<String>,
}

impl GroupDiscoveryQuery {
    pub fn new(
        days: i32,
        kind: GroupDiscoveryKind,
        after: Option<String>,
    ) -> Result<Self, DiscoveryError> {
        validate_period(days)?;
        let has_cursor = after.is_some();
        let after = normalize_label(after)?;
        if has_cursor && after.is_none() {
            return Err(DiscoveryError::Invalid);
        }
        Ok(Self { days, kind, after })
    }

    pub fn validate(&self) -> Result<(), DiscoveryError> {
        Self::new(self.days, self.kind, self.after.clone()).map(|_| ())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupDiscoveryRow {
    /// None means the current book metadata has no value of this kind.
    pub label: Option<String>,
    pub matched_books: i64,
    pub current: BookDiscoveryMetrics,
    pub previous: BookDiscoveryMetrics,
    pub current_counts: BookDiscoveryCounts,
    pub previous_counts: BookDiscoveryCounts,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GroupDiscoveryReport {
    pub days: i32,
    pub kind: GroupDiscoveryKind,
    pub measured_at_utc: String,
    /// Unique public books across the whole catalog, never summed group totals.
    pub current: BookDiscoveryPeriod,
    pub previous: BookDiscoveryPeriod,
    pub health: DiscoveryHealth,
    pub rows: Vec<GroupDiscoveryRow>,
    pub next_after: Option<String>,
    pub matched_groups: i64,
    pub matched_books: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_cursor_is_bounded_and_cannot_alias_the_missing_group() {
        assert_eq!(
            GroupDiscoveryQuery::new(7, GroupDiscoveryKind::Tags, Some("  Два  мира ".into()))
                .unwrap()
                .after
                .as_deref(),
            Some("два мира")
        );
        for cursor in ["".into(), "  ".into(), "x".repeat(41), "a\nb".into()] {
            assert_eq!(
                GroupDiscoveryQuery::new(7, GroupDiscoveryKind::Genres, Some(cursor)),
                Err(DiscoveryError::Invalid)
            );
        }
        assert!(GroupDiscoveryQuery::new(7, GroupDiscoveryKind::Genres, None).is_ok());
        assert_eq!(
            GroupDiscoveryQuery::new(2, GroupDiscoveryKind::Genres, None),
            Err(DiscoveryError::Invalid)
        );
    }
}
