//! Saved public-page counts by the UTC hour of RIL batch persistence.
//! Missing rows are unknown, never evidence of zero traffic or complete capture.
use std::{error::Error, fmt};

pub const FLOW_AUDIENCE_DAYS: [i32; 2] = [1, 7];
pub const FLOW_AUDIENCE_SURFACES: usize = 4;
const HOUR_SECONDS: i64 = 3_600;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FlowHourlyError {
    Invalid,
    Unavailable,
}

impl fmt::Display for FlowHourlyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl Error for FlowHourlyError {}

pub fn validate_period(days: i32) -> Result<usize, FlowHourlyError> {
    match days {
        1 => Ok(24),
        7 => Ok(168),
        _ => Err(FlowHourlyError::Invalid),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlowHour {
    pub hour_epoch: i64,
    pub hour_utc: String,
    /// Home, catalogue, book page, reader. None means no persisted row.
    pub views: [Option<i64>; FLOW_AUDIENCE_SURFACES],
    /// Requests whose existing consent was verified by the server.
    pub opted_in_html_requests: [Option<i64>; FLOW_AUDIENCE_SURFACES],
    /// Observed process-local visit starts, never unique people.
    pub observed_visit_starts: [Option<i64>; FLOW_AUDIENCE_SURFACES],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlowHourlyPeriod {
    /// Inclusive start, exclusive end; both exact UTC hour boundaries.
    pub from_epoch: i64,
    pub until_epoch: i64,
    pub from_utc: String,
    pub until_utc: String,
    pub hours: Vec<FlowHour>,
    /// Sums of saved observations only; not estimates for missing hours.
    pub saved_views: [Option<i64>; FLOW_AUDIENCE_SURFACES],
    /// Subsets of saved public HTML observations; not a site-wide audience.
    pub saved_opted_in_requests: [Option<i64>; FLOW_AUDIENCE_SURFACES],
    pub observed_visit_starts: [Option<i64>; FLOW_AUDIENCE_SURFACES],
    /// Hours containing a row, not a measure of collection completeness.
    pub recorded_hours: [u16; FLOW_AUDIENCE_SURFACES],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FlowHourlyReport {
    pub days: i32,
    pub measured_at_epoch: i64,
    pub measured_at_utc: String,
    pub current: FlowHourlyPeriod,
    pub previous: FlowHourlyPeriod,
}

pub(crate) struct ReportAnchor {
    pub measured_at_epoch: i64,
    pub measured_at_utc: String,
    pub until_epoch: i64,
    pub until_utc: String,
    pub current_from_utc: String,
    pub previous_from_utc: String,
}

pub(crate) fn report_from_rows(
    days: i32,
    anchor: ReportAnchor,
    mut rows: Vec<FlowHour>,
) -> Result<FlowHourlyReport, FlowHourlyError> {
    let hours = validate_period(days)?;
    let span = hours as i64 * HOUR_SECONDS;
    let current_from = anchor
        .until_epoch
        .checked_sub(span)
        .ok_or(FlowHourlyError::Unavailable)?;
    let previous_from = current_from
        .checked_sub(span)
        .ok_or(FlowHourlyError::Unavailable)?;
    if anchor.until_epoch.rem_euclid(HOUR_SECONDS) != 0
        || anchor.measured_at_epoch < anchor.until_epoch
        || anchor
            .measured_at_epoch
            .checked_sub(anchor.until_epoch)
            .is_none_or(|n| n >= HOUR_SECONDS)
        || rows.len() != hours * 2
    {
        return Err(FlowHourlyError::Unavailable);
    }
    for (index, row) in rows.iter().enumerate() {
        let invalid_counters = (0..FLOW_AUDIENCE_SURFACES).any(|surface| {
            match (
                row.views[surface],
                row.opted_in_html_requests[surface],
                row.observed_visit_starts[surface],
            ) {
                (None, None, None) => false,
                (Some(views), Some(opted), Some(starts)) => {
                    views < 0 || opted < 0 || starts < 0 || starts > opted || opted > views
                }
                _ => true,
            }
        });
        if row.hour_epoch != previous_from + index as i64 * HOUR_SECONDS || invalid_counters {
            return Err(FlowHourlyError::Unavailable);
        }
    }
    let current_rows = rows.split_off(hours);
    let current = period_from_hours(
        current_from,
        anchor.until_epoch,
        anchor.current_from_utc.clone(),
        anchor.until_utc,
        current_rows,
    )?;
    let previous = period_from_hours(
        previous_from,
        current_from,
        anchor.previous_from_utc,
        anchor.current_from_utc,
        rows,
    )?;
    Ok(FlowHourlyReport {
        days,
        measured_at_epoch: anchor.measured_at_epoch,
        measured_at_utc: anchor.measured_at_utc,
        current,
        previous,
    })
}

fn period_from_hours(
    from_epoch: i64,
    until_epoch: i64,
    from_utc: String,
    until_utc: String,
    hours: Vec<FlowHour>,
) -> Result<FlowHourlyPeriod, FlowHourlyError> {
    let mut saved_views: [Option<i64>; FLOW_AUDIENCE_SURFACES] = [None; FLOW_AUDIENCE_SURFACES];
    let mut saved_opted_in_requests = [None; FLOW_AUDIENCE_SURFACES];
    let mut observed_visit_starts = [None; FLOW_AUDIENCE_SURFACES];
    let mut recorded_hours = [0; FLOW_AUDIENCE_SURFACES];
    for hour in &hours {
        for (surface, views) in hour.views.iter().enumerate() {
            if let Some(views) = views {
                saved_views[surface] = Some(
                    saved_views[surface]
                        .unwrap_or(0)
                        .checked_add(*views)
                        .ok_or(FlowHourlyError::Unavailable)?,
                );
                let opted = hour.opted_in_html_requests[surface]
                    .ok_or(FlowHourlyError::Unavailable)?;
                let starts = hour.observed_visit_starts[surface]
                    .ok_or(FlowHourlyError::Unavailable)?;
                saved_opted_in_requests[surface] = Some(
                    saved_opted_in_requests[surface]
                        .unwrap_or(0_i64)
                        .checked_add(opted)
                        .ok_or(FlowHourlyError::Unavailable)?,
                );
                observed_visit_starts[surface] = Some(
                    observed_visit_starts[surface]
                        .unwrap_or(0_i64)
                        .checked_add(starts)
                        .ok_or(FlowHourlyError::Unavailable)?,
                );
                recorded_hours[surface] += 1;
            }
        }
    }
    Ok(FlowHourlyPeriod {
        from_epoch,
        until_epoch,
        from_utc,
        until_utc,
        hours,
        saved_views,
        saved_opted_in_requests,
        observed_visit_starts,
        recorded_hours,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNTIL: i64 = 1_800_000_000;

    fn anchor() -> ReportAnchor {
        ReportAnchor {
            measured_at_epoch: UNTIL + 42,
            until_epoch: UNTIL,
            measured_at_utc: "2027-01-15T08:00:42Z".into(),
            until_utc: "2027-01-15T08:00:00Z".into(),
            current_from_utc: "2027-01-08T08:00:00Z".into(),
            previous_from_utc: "2027-01-01T08:00:00Z".into(),
        }
    }

    fn empty_hours(days: i32) -> Vec<FlowHour> {
        let hours = validate_period(days).unwrap() as i64 * 2;
        (0..hours)
            .map(|index| FlowHour {
                hour_epoch: UNTIL - (hours - index) * HOUR_SECONDS,
                hour_utc: "2027-01-01T08:00:00Z".into(),
                views: [None; 4],
                opted_in_html_requests: [None; 4],
                observed_visit_starts: [None; 4],
            })
            .collect()
    }

    #[test]
    fn missing_surface_hours_and_measured_zero_are_distinct() {
        let mut rows = empty_hours(7);
        rows[335].views[0] = Some(0);
        rows[335].opted_in_html_requests[0] = Some(0);
        rows[335].observed_visit_starts[0] = Some(0);
        rows[168].views[1] = Some(7);
        rows[168].opted_in_html_requests[1] = Some(3);
        rows[168].observed_visit_starts[1] = Some(1);
        rows[167].views[2] = Some(11);
        rows[167].opted_in_html_requests[2] = Some(0);
        rows[167].observed_visit_starts[2] = Some(0);
        rows[0].views[3] = Some(13);
        rows[0].opted_in_html_requests[3] = Some(5);
        rows[0].observed_visit_starts[3] = Some(2);
        let report = report_from_rows(7, anchor(), rows).unwrap();
        assert_eq!(report.current.hours.len(), 168);
        assert_eq!(report.previous.hours.len(), 168);
        assert_eq!(report.current.hours[167].views, [Some(0), None, None, None]);
        assert_eq!(report.current.hours[0].views, [None, Some(7), None, None]);
        assert_eq!(report.previous.hours[0].views, [None, None, None, Some(13)]);
        assert_eq!(report.current.saved_views, [Some(0), Some(7), None, None]);
        assert_eq!(
            report.previous.saved_views,
            [None, None, Some(11), Some(13)]
        );
        assert_eq!(report.current.saved_opted_in_requests, [Some(0), Some(3), None, None]);
        assert_eq!(report.current.observed_visit_starts, [Some(0), Some(1), None, None]);
        assert_eq!(report.previous.saved_opted_in_requests, [None, None, Some(0), Some(5)]);
        assert_eq!(report.previous.observed_visit_starts, [None, None, Some(0), Some(2)]);
        assert_eq!(report.current.recorded_hours, [1, 1, 0, 0]);
        assert_eq!(report.previous.recorded_hours, [0, 0, 1, 1]);
        assert_eq!(report.current.from_epoch, report.previous.until_epoch);
    }

    #[test]
    fn empty_periods_stay_unknown_and_only_short_retained_comparisons_are_allowed() {
        for days in FLOW_AUDIENCE_DAYS {
            let report = report_from_rows(days, anchor(), empty_hours(days)).unwrap();
            assert_eq!(report.current.saved_views, [None; 4]);
            assert_eq!(report.current.saved_opted_in_requests, [None; 4]);
            assert_eq!(report.previous.observed_visit_starts, [None; 4]);
            assert_eq!(report.previous.recorded_hours, [0; 4]);
            assert_eq!(report.current.hours.len(), (days * 24) as usize);
        }
        for days in [i32::MIN, -1, 0, 2, 30, 90, i32::MAX] {
            assert_eq!(validate_period(days), Err(FlowHourlyError::Invalid));
        }
    }

    #[test]
    fn invalid_rows_duplicates_and_overflow_fail_without_partial_totals() {
        for bad_epoch in [UNTIL, UNTIL - 49 * HOUR_SECONDS, UNTIL - 1] {
            let mut rows = empty_hours(1);
            rows[0].hour_epoch = bad_epoch;
            assert_eq!(
                report_from_rows(1, anchor(), rows),
                Err(FlowHourlyError::Unavailable)
            );
        }
        let mut negative = empty_hours(1);
        negative[0].views[0] = Some(-1);
        negative[0].opted_in_html_requests[0] = Some(0);
        negative[0].observed_visit_starts[0] = Some(0);
        let mut duplicate = empty_hours(1);
        duplicate[1] = duplicate[0].clone();
        let mut overflow = empty_hours(1);
        overflow[0].views[0] = Some(i64::MAX);
        overflow[0].opted_in_html_requests[0] = Some(0);
        overflow[0].observed_visit_starts[0] = Some(0);
        overflow[1].views[0] = Some(1);
        overflow[1].opted_in_html_requests[0] = Some(0);
        overflow[1].observed_visit_starts[0] = Some(0);
        let mut invalid_opted = empty_hours(1);
        invalid_opted[0].views[0] = Some(2);
        invalid_opted[0].opted_in_html_requests[0] = Some(3);
        invalid_opted[0].observed_visit_starts[0] = Some(1);
        let mut invalid_visit = empty_hours(1);
        invalid_visit[0].views[0] = Some(3);
        invalid_visit[0].opted_in_html_requests[0] = Some(1);
        invalid_visit[0].observed_visit_starts[0] = Some(2);
        for rows in [negative, duplicate, overflow, invalid_opted, invalid_visit, Vec::new()] {
            assert_eq!(
                report_from_rows(1, anchor(), rows),
                Err(FlowHourlyError::Unavailable)
            );
        }
        for (measured_at, until) in [
            (UNTIL - 1, UNTIL),
            (UNTIL + 3600, UNTIL),
            (UNTIL, UNTIL - 1),
            (i64::MIN, i64::MIN),
        ] {
            let mut bad_anchor = anchor();
            bad_anchor.measured_at_epoch = measured_at;
            bad_anchor.until_epoch = until;
            assert_eq!(
                report_from_rows(1, bad_anchor, empty_hours(1)),
                Err(FlowHourlyError::Unavailable)
            );
        }
    }
}
