//! Closed-hour distributions of retained, verified C chapter facts.
//! These are neither people nor first account/book completions.
use super::{validate_period, AudienceError};

pub const READING_TIME_ZONES: [&str; 4] = [
    "UTC",
    "Europe/Moscow",
    "Asia/Yekaterinburg",
    "Europe/Amsterdam",
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReadingTimeCounts {
    pub starts: i64,
    pub chapter_completions: i64,
}

impl ReadingTimeCounts {
    fn checked_add(self, rhs: Self) -> Result<Self, AudienceError> {
        Ok(Self {
            starts: self
                .starts
                .checked_add(rhs.starts)
                .ok_or(AudienceError::Unavailable)?,
            chapter_completions: self
                .chapter_completions
                .checked_add(rhs.chapter_completions)
                .ok_or(AudienceError::Unavailable)?,
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadingTimeHour {
    /// Inclusive UTC hour; the following 3600 seconds form one slot.
    pub epoch: i64,
    /// Local YYYY-MM-DD HH:00. Repeated labels retain distinct epoch/offset.
    pub local_label: String,
    pub offset_minutes: i32,
    /// Monday=0 through Sunday=6.
    pub weekday: usize,
    pub local_hour: usize,
    /// None if any retained row in this slot lacks its matching C proof.
    pub counts: Option<ReadingTimeCounts>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ReadingTimeCell {
    /// Actual UTC slots in this local category, including repeated DST hours.
    pub utc_hours: i32,
    /// utc_hours=0 is outside the window; otherwise None means unknown proof.
    pub counts: Option<ReadingTimeCounts>,
}

impl ReadingTimeCell {
    fn include(&mut self, counts: Option<ReadingTimeCounts>) -> Result<(), AudienceError> {
        self.counts = if self.utc_hours == 0 {
            counts
        } else {
            self.counts
                .zip(counts)
                .map(|(a, b)| a.checked_add(b))
                .transpose()?
        };
        self.utc_hours += 1;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadingTimePeriod {
    pub from_epoch: i64,
    pub until_epoch: i64,
    pub from_utc: String,
    pub until_utc: String,
    pub totals: Option<ReadingTimeCounts>,
    /// Exactly days*24 slots, oldest first; never coalesced by local label.
    pub hours: Vec<ReadingTimeHour>,
    pub by_hour: [ReadingTimeCell; 24],
    pub by_weekday: [ReadingTimeCell; 7],
}

impl ReadingTimePeriod {
    pub(crate) fn summarize(&mut self) -> Result<(), AudienceError> {
        let mut total = ReadingTimeCell::default();
        for (index, hour) in self.hours.iter().enumerate() {
            if hour.epoch != self.from_epoch + index as i64 * 3600
                || hour.weekday >= 7
                || hour.local_hour >= 24
                || hour
                    .counts
                    .is_some_and(|c| c.starts < 0 || c.chapter_completions < 0)
            {
                return Err(AudienceError::Unavailable);
            }
            total.include(hour.counts)?;
            self.by_hour[hour.local_hour].include(hour.counts)?;
            self.by_weekday[hour.weekday].include(hour.counts)?;
        }
        if self.from_epoch + i64::from(total.utc_hours) * 3600 != self.until_epoch {
            return Err(AudienceError::Unavailable);
        }
        self.totals = total.counts;
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReadingTimeReport {
    pub days: i32,
    pub time_zone: String,
    pub measured_at_utc: String,
    /// Exclusive boundary of the latest fully closed UTC hour.
    pub until_epoch: i64,
    pub current: ReadingTimePeriod,
    pub previous: ReadingTimePeriod,
}

pub(crate) fn validate(days: i32, zone: &str) -> Result<(), AudienceError> {
    validate_period(days)?;
    if READING_TIME_ZONES.contains(&zone) {
        Ok(())
    } else {
        Err(AudienceError::Invalid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_is_not_a_partial_sum_or_a_slot_outside_the_window() {
        let mut cell = ReadingTimeCell::default();
        assert_eq!(cell.utc_hours, 0);
        assert_eq!(cell.counts, None);
        cell.include(Some(ReadingTimeCounts::default())).unwrap();
        assert_eq!(cell.utc_hours, 1);
        assert_eq!(cell.counts, Some(ReadingTimeCounts::default()));
        cell.include(None).unwrap();
        cell.include(Some(ReadingTimeCounts {
            starts: 3,
            chapter_completions: 2,
        }))
        .unwrap();
        assert_eq!(cell.utc_hours, 3);
        assert_eq!(cell.counts, None);
    }

    #[test]
    fn only_bounded_periods_and_the_explicit_whole_hour_zones_are_accepted() {
        for days in [1, 7, 30, 90] {
            for zone in READING_TIME_ZONES {
                assert_eq!(validate(days, zone), Ok(()));
            }
        }
        for days in [i32::MIN, 0, 2, 91, i32::MAX] {
            assert_eq!(validate(days, "UTC"), Err(AudienceError::Invalid));
        }
        for zone in [
            "",
            "utc",
            "Asia/Kathmandu",
            "Europe/London",
            "UTC; SELECT 1",
        ] {
            assert_eq!(validate(7, zone), Err(AudienceError::Invalid));
        }
    }
}
