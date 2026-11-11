//! Anonymous observation hours; missing counts are never inferred zeroes.
use super::audience::FlowHourlyError;

pub const OBSERVED_ZONES: [&str; 4] = [
    "UTC",
    "Europe/Moscow",
    "Asia/Yekaterinburg",
    "Europe/Amsterdam",
];

pub fn validate_zone(zone: &str) -> Result<&'static str, FlowHourlyError> {
    OBSERVED_ZONES
        .iter()
        .copied()
        .find(|known| *known == zone)
        .ok_or(FlowHourlyError::Invalid)
}

pub fn observed_hours(days: i32) -> Result<usize, FlowHourlyError> {
    match days {
        1 | 7 | 30 | 90 => Ok(days as usize * 24),
        _ => Err(FlowHourlyError::Invalid),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedHour {
    pub epoch: i64,
    pub local_label: String,
    pub offset_minutes: i32,
    /// Monday=0; local clock repeats at DST transitions remain separate UTC rows.
    pub weekday: usize,
    pub local_hour: usize,
    pub views: [Option<i64>; 4],
    pub sources: [Option<i64>; 4],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedPeriod {
    pub from_epoch: i64,
    pub until_epoch: i64,
    pub from_utc: String,
    pub until_utc: String,
    pub hours: Vec<ObservedHour>,
    pub surfaces: [Option<i64>; 4],
    pub sources: [Option<i64>; 4],
    pub saved_total: Option<i64>,
    pub recorded_hours: usize,
    pub weekday_hour: [[Option<i64>; 24]; 7],
    /// Presence only, not collection completeness or estimated traffic.
    pub recorded_slots: [[u16; 24]; 7],
    pub expected_slots: [[u16; 24]; 7],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedAudienceReport {
    pub days: i32,
    pub time_zone: String,
    pub measured_at_utc: String,
    pub current: ObservedPeriod,
    pub previous: ObservedPeriod,
    /// Discards by materialization time, not by the observation-hour interval.
    pub persisted_discards: i64,
}

pub(crate) struct ObservedAnchor {
    pub measured_at_utc: String,
    pub until_epoch: i64,
    pub until_utc: String,
    pub current_from_utc: String,
    pub previous_from_utc: String,
    pub persisted_discards: i64,
}

fn add(target: &mut Option<i64>, value: Option<i64>) -> Result<(), FlowHourlyError> {
    if let Some(value) = value {
        if value < 0 {
            return Err(FlowHourlyError::Unavailable);
        }
        *target = Some(
            target
                .unwrap_or(0)
                .checked_add(value)
                .ok_or(FlowHourlyError::Unavailable)?,
        );
    }
    Ok(())
}

fn total(values: &[Option<i64>; 4]) -> Result<Option<i64>, FlowHourlyError> {
    let mut total = None;
    for value in values {
        add(&mut total, *value)?;
    }
    Ok(total)
}

pub(crate) fn observed_report(
    days: i32,
    zone: &str,
    anchor: ObservedAnchor,
    mut rows: Vec<ObservedHour>,
) -> Result<ObservedAudienceReport, FlowHourlyError> {
    let count = observed_hours(days)?;
    validate_zone(zone)?;
    let span = count as i64 * 3_600;
    let from = anchor
        .until_epoch
        .checked_sub(2 * span)
        .ok_or(FlowHourlyError::Unavailable)?;
    if anchor.until_epoch.rem_euclid(3_600) != 0
        || rows.len() != 2 * count
        || anchor.persisted_discards < 0
    {
        return Err(FlowHourlyError::Unavailable);
    }
    for (index, hour) in rows.iter().enumerate() {
        if hour.epoch != from + index as i64 * 3_600
            || hour.weekday > 6
            || hour.local_hour > 23
            || !(-840..=840).contains(&hour.offset_minutes)
            || total(&hour.views)? != total(&hour.sources)?
        {
            return Err(FlowHourlyError::Unavailable);
        }
    }
    let current_rows = rows.split_off(count);
    let previous = period(
        from,
        anchor.until_epoch - span,
        anchor.previous_from_utc,
        anchor.current_from_utc.clone(),
        rows,
    )?;
    let current = period(
        anchor.until_epoch - span,
        anchor.until_epoch,
        anchor.current_from_utc,
        anchor.until_utc,
        current_rows,
    )?;
    Ok(ObservedAudienceReport {
        days,
        time_zone: zone.into(),
        measured_at_utc: anchor.measured_at_utc,
        current,
        previous,
        persisted_discards: anchor.persisted_discards,
    })
}

fn period(
    from_epoch: i64,
    until_epoch: i64,
    from_utc: String,
    until_utc: String,
    hours: Vec<ObservedHour>,
) -> Result<ObservedPeriod, FlowHourlyError> {
    let mut result = ObservedPeriod {
        from_epoch,
        until_epoch,
        from_utc,
        until_utc,
        hours,
        surfaces: [None; 4],
        sources: [None; 4],
        saved_total: None,
        recorded_hours: 0,
        weekday_hour: [[None; 24]; 7],
        recorded_slots: [[0; 24]; 7],
        expected_slots: [[0; 24]; 7],
    };
    for hour in &result.hours {
        for index in 0..4 {
            add(&mut result.surfaces[index], hour.views[index])?;
            add(&mut result.sources[index], hour.sources[index])?;
        }
        let value = total(&hour.views)?;
        result.expected_slots[hour.weekday][hour.local_hour] += 1;
        if value.is_some() {
            result.recorded_hours += 1;
            result.recorded_slots[hour.weekday][hour.local_hour] += 1;
            add(
                &mut result.weekday_hour[hour.weekday][hour.local_hour],
                value,
            )?;
            add(&mut result.saved_total, value)?;
        }
    }
    Ok(result)
}

#[cfg(test)]
#[path = "observed_audience/tests.rs"]
mod tests;
