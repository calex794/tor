//! C-09/S006: version-pinned, revision-checked reading positions.
#![forbid(unsafe_code)]
use super::progress::ReadingPlace;
use tor_contracts::PositiveId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncError {
    Invalid,
    NotFound,
    Conflict,
    Unavailable,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncReceipt {
    pub revision: i64,
    pub updated_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoredProgress {
    pub chapter_id: i64,
    pub version_id: i64,
    pub place: ReadingPlace,
    pub receipt: SyncReceipt,
}

/// New JSON commands use decimal strings to avoid JavaScript integer loss.
pub fn positive_id(raw: &str) -> Result<i64, SyncError> {
    raw.parse::<PositiveId>()
        .map(PositiveId::get)
        .map_err(|_| SyncError::Invalid)
}

pub fn checked_place(place: ReadingPlace, revision: i64) -> Result<(), SyncError> {
    if revision < 0
        || !(1..=200).contains(&place.block_index)
        || !(0..=100_000).contains(&place.char_offset)
    {
        return Err(SyncError::Invalid);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_lossless_and_signed_values_are_rejected() {
        assert_eq!(positive_id("9223372036854775807"), Ok(i64::MAX));
        for candidate in ["", "0", "-1", "+2", "1.2", " 1", "18446744073709551615"] {
            assert_eq!(
                positive_id(candidate),
                Err(SyncError::Invalid),
                "{candidate}"
            );
        }
    }

    #[test]
    fn place_and_revision_validation_is_bounded() {
        assert_eq!(
            checked_place(
                ReadingPlace {
                    block_index: 1,
                    char_offset: 0
                },
                0
            ),
            Ok(())
        );
        assert_eq!(
            checked_place(
                ReadingPlace {
                    block_index: 200,
                    char_offset: 100_000
                },
                1
            ),
            Ok(())
        );
        assert_eq!(
            checked_place(
                ReadingPlace {
                    block_index: 1,
                    char_offset: 0
                },
                -1
            ),
            Err(SyncError::Invalid)
        );
        assert_eq!(
            checked_place(
                ReadingPlace {
                    block_index: 201,
                    char_offset: 0
                },
                0
            ),
            Err(SyncError::Invalid)
        );
    }
}
