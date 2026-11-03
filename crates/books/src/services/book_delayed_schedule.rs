//! NEwBOO's book-level publication cadence is separate from per-chapter UTC release.
//! Pure validation; no publication/reader entitlement is granted by these values.
use super::authoring::AuthorError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BookDelayedSchedule {
    pub enabled: bool,
    pub interval_days: i32,
    pub publication_times: Vec<String>,
    pub schedule_revision: i64,
    pub anchor_date: Option<String>,
    pub next_slot_utc: Option<String>,
    pub next_slot_index: Option<i32>,
}

impl Default for BookDelayedSchedule {
    fn default() -> Self {
        Self {
            enabled: false,
            interval_days: 1,
            publication_times: Vec::new(),
            schedule_revision: 0,
            anchor_date: None,
            next_slot_utc: None,
            next_slot_index: None,
        }
    }
}

/// NEwBOO: 1-100 sorted minute-precision Moscow slots, duplicates allowed.
pub fn parse_publication_times(values: &[String]) -> Result<Vec<i32>, AuthorError> {
    if !(1..=100).contains(&values.len()) {
        return Err(AuthorError::Invalid);
    }
    let mut minutes = Vec::with_capacity(values.len());
    for value in values {
        let bytes = value.as_bytes();
        if bytes.len() != 5
            || bytes[2] != b':'
            || !bytes[0].is_ascii_digit()
            || !bytes[1].is_ascii_digit()
            || !bytes[3].is_ascii_digit()
            || !bytes[4].is_ascii_digit()
        {
            return Err(AuthorError::Invalid);
        }
        let hours = i32::from(bytes[0] - b'0') * 10 + i32::from(bytes[1] - b'0');
        let minute = i32::from(bytes[3] - b'0') * 10 + i32::from(bytes[4] - b'0');
        if hours > 23 || minute > 59 {
            return Err(AuthorError::Invalid);
        }
        let total = hours * 60 + minute;
        if minutes.last().is_some_and(|previous| *previous > total) {
            return Err(AuthorError::Invalid);
        }
        minutes.push(total);
    }
    Ok(minutes)
}

pub fn publication_time_text(minute: i32) -> Result<String, AuthorError> {
    if !(0..=1439).contains(&minute) {
        return Err(AuthorError::Invalid);
    }
    Ok(format!("{:02}:{:02}", minute / 60, minute % 60))
}

/// Preserve the original cadence anchor when only slot times change. This
/// returns the first on-cadence Moscow calendar day at or after today.
pub fn on_cadence_day(anchor: i32, today: i32, interval_days: i32) -> Result<i32, AuthorError> {
    if interval_days < 1 {
        return Err(AuthorError::Invalid);
    }
    if anchor >= today {
        return Ok(anchor);
    }
    let elapsed = i64::from(today) - i64::from(anchor);
    let interval = i64::from(interval_days);
    i32::try_from(i64::from(anchor) + ((elapsed + interval - 1) / interval) * interval)
        .map_err(|_| AuthorError::Invalid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slots(input: &[&str]) -> Vec<String> {
        input.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn s058_slot_validation_matches_neboo() {
        assert_eq!(
            parse_publication_times(&slots(&["09:00", "09:00", "23:59"])),
            Ok(vec![540, 540, 1439])
        );
        for invalid in [
            slots(&[]),
            slots(&["9:00"]),
            slots(&["24:00"]),
            slots(&["12:60"]),
            slots(&["10:00", "09:59"]),
            slots(&["12:00 "]),
        ] {
            assert_eq!(parse_publication_times(&invalid), Err(AuthorError::Invalid));
        }
        assert_eq!(parse_publication_times(&slots(&vec!["00:00"; 100])).unwrap().len(), 100);
        assert_eq!(parse_publication_times(&slots(&vec!["00:00"; 101])), Err(AuthorError::Invalid));
        assert_eq!(publication_time_text(1439).unwrap(), "23:59");
        assert_eq!(publication_time_text(-1), Err(AuthorError::Invalid));
    }

    #[test]
    fn s058_cadence_alignment_skips_off_days() {
        assert_eq!(on_cadence_day(100, 100, 3), Ok(100));
        assert_eq!(on_cadence_day(100, 101, 3), Ok(103));
        assert_eq!(on_cadence_day(100, 106, 3), Ok(106));
        assert_eq!(on_cadence_day(100, 98, 3), Ok(100));
        assert_eq!(on_cadence_day(100, 105, 0), Err(AuthorError::Invalid));
    }
}
