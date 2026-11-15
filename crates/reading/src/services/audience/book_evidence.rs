//! Retained C chapter evidence for a bounded set of currently public books.
//! Zero is about these retained canonical facts, never all reading activity.
use super::{validate_period, AudienceError};

pub const BOOK_EVIDENCE_LIMIT: usize = 50;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ClosedChapterCounts {
    /// First verified start per account/book/chapter, not visits or book starts.
    pub starts: i64,
    /// All retained verified chapter completions in the interval; later chapters
    /// of an already-known account/book pair are included, repeat PUTs are not.
    pub chapter_completions: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosedBookEvidenceRow {
    pub book_id: i64,
    /// None when the book is absent/ineligible or retained C evidence in this
    /// period lacks its exact matching outbox proof. Never coalesce it to zero.
    pub current: Option<ClosedChapterCounts>,
    pub previous: Option<ClosedChapterCounts>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClosedBookEvidenceReport {
    pub days: i32,
    /// Caller-supplied exclusive UTC-hour boundary, validated against DB time.
    pub until_epoch: i64,
    pub measured_at_utc: String,
    /// Requested IDs once each in ascending order, including unknown entries.
    pub rows: Vec<ClosedBookEvidenceRow>,
}

pub(crate) fn validate(
    days: i32,
    until_epoch: i64,
    book_ids: &[i64],
) -> Result<Vec<i64>, AudienceError> {
    validate_period(days)?;
    if until_epoch <= 0
        || until_epoch % 3600 != 0
        || book_ids.len() > BOOK_EVIDENCE_LIMIT
        || book_ids.iter().any(|id| *id <= 0)
    {
        return Err(AudienceError::Invalid);
    }
    let mut ids = book_ids.to_vec();
    ids.sort_unstable();
    ids.dedup();
    Ok(ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_raw_bounds_before_dedup_and_keep_only_public_book_keys() {
        assert_eq!(validate(7, 3600, &[3, 1, 3]).unwrap(), [1, 3]);
        assert!(validate(90, 3600, &[]).unwrap().is_empty());
        for days in [0, 2, 91] {
            assert_eq!(validate(days, 3600, &[1]), Err(AudienceError::Invalid));
        }
        for until in [i64::MIN, -3600, 0, 3599, 3601] {
            assert_eq!(validate(1, until, &[1]), Err(AudienceError::Invalid));
        }
        assert_eq!(validate(1, 3600, &[1; 51]), Err(AudienceError::Invalid));
        assert_eq!(validate(1, 3600, &[0]), Err(AudienceError::Invalid));
        assert!(validate(1, 3600, &[1; 50]).is_ok());
    }
}
