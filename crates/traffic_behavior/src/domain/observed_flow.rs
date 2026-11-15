//! Anonymous public HTML counts by the server's observation hour.
//! No subject, session, account, raw URL or fine-grained timestamp is retained.
use super::{
    flow::{acquisition_index, surface_index},
    FlowPageview,
};
use std::{
    collections::BTreeMap,
    time::{SystemTime, UNIX_EPOCH},
};

pub const MAX_OBSERVED_HOURS: usize = 48;
const MAX_CELL: u64 = 1_000_000_000;
const LAST_HOUR: i64 = 253_402_297_200;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObservedFlowCount {
    pub hour_epoch: i64,
    pub surface: i16,
    pub acquisition_hint: i16,
    pub html_requests: i64,
}

#[derive(Clone, Default)]
pub(super) struct ObservedFlowDelta {
    hours: BTreeMap<i64, [[u64; 4]; 4]>,
    discarded: u64,
}

/// Called before queue insertion with a server clock, never browser input.
pub fn observation_hour(clock: SystemTime) -> Option<i64> {
    let seconds = i64::try_from(clock.duration_since(UNIX_EPOCH).ok()?.as_secs()).ok()?;
    let hour = seconds - seconds.rem_euclid(3_600);
    (hour <= LAST_HOUR).then_some(hour)
}

impl ObservedFlowDelta {
    /// None is an excluded non-public surface; false is a discarded public count.
    pub fn record(&mut self, page: FlowPageview, hour: Option<i64>) -> Option<bool> {
        let surface = surface_index(page.surface).filter(|index| *index < 4)?;
        let accepted = hour
            .filter(|hour| {
                (0..=LAST_HOUR).contains(hour)
                    && hour.rem_euclid(3_600) == 0
                    && (self.hours.contains_key(hour) || self.hours.len() < MAX_OBSERVED_HOURS)
            })
            .is_some_and(|hour| {
                let cell = &mut self.hours.entry(hour).or_default()[surface]
                    [acquisition_index(page.acquisition_hint)];
                if *cell >= MAX_CELL {
                    return false;
                }
                *cell += 1;
                true
            });
        if !accepted {
            self.discarded = self.discarded.saturating_add(1);
        }
        Some(accepted)
    }

    pub fn rows(&self) -> Vec<ObservedFlowCount> {
        let mut rows = Vec::new();
        for (hour, surfaces) in &self.hours {
            for (surface, sources) in surfaces.iter().enumerate() {
                for (source, count) in sources.iter().enumerate() {
                    if *count > 0 {
                        rows.push(ObservedFlowCount {
                            hour_epoch: *hour,
                            surface: surface as i16,
                            acquisition_hint: source as i16,
                            html_requests: *count as i64,
                        });
                    }
                }
            }
        }
        rows
    }

    pub fn discarded(&self) -> u64 {
        self.discarded
    }
    pub fn is_empty(&self) -> bool {
        self.hours.is_empty() && self.discarded == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tor_contracts::flow::{AcquisitionChannel, RouteSurface};

    fn page() -> FlowPageview {
        FlowPageview {
            surface: RouteSurface::Home,
            acquisition_hint: AcquisitionChannel::DirectOrUnknown,
        }
    }

    #[test]
    fn observation_time_precedes_queue_and_midnight_never_moves_old_counts() {
        let start = 1_800_000_000;
        let old = observation_hour(UNIX_EPOCH + Duration::from_secs(start - 1));
        let next = observation_hour(UNIX_EPOCH + Duration::from_secs(start));
        assert_eq!(old, Some(start as i64 - 3_600));
        assert_eq!(next, Some(start as i64));
        let mut delta = ObservedFlowDelta::default();
        assert_eq!(delta.record(page(), old), Some(true));
        assert_eq!(delta.record(page(), next), Some(true));
        assert_eq!(delta.record(page(), old), Some(true));
        let rows = delta.rows();
        assert_eq!(
            (rows[0].hour_epoch, rows[0].html_requests),
            (old.unwrap(), 2)
        );
        assert_eq!(
            (rows[1].hour_epoch, rows[1].html_requests),
            (next.unwrap(), 1)
        );
    }

    #[test]
    fn clock_errors_capacity_and_counter_overflow_are_explicit_losses() {
        let mut delta = ObservedFlowDelta::default();
        for hour in 0..MAX_OBSERVED_HOURS {
            assert_eq!(delta.record(page(), Some(hour as i64 * 3_600)), Some(true));
        }
        for hour in [
            None,
            Some(-3_600),
            Some(1),
            Some(LAST_HOUR + 3_600),
            Some(MAX_OBSERVED_HOURS as i64 * 3_600),
        ] {
            assert_eq!(delta.record(page(), hour), Some(false));
        }
        assert_eq!(delta.hours.len(), MAX_OBSERVED_HOURS);
        delta.hours.get_mut(&0).unwrap()[0][3] = MAX_CELL;
        assert_eq!(delta.record(page(), Some(0)), Some(false));
        assert_eq!(delta.discarded(), 6);
        assert_eq!(delta.rows()[0].html_requests, MAX_CELL as i64);
        assert_eq!(observation_hour(UNIX_EPOCH - Duration::from_secs(1)), None);
    }

    #[test]
    fn private_api_and_unknown_surfaces_never_enter_public_hour_counts() {
        let mut delta = ObservedFlowDelta::default();
        for surface in [
            RouteSurface::AuthenticatedPage,
            RouteSurface::Api,
            RouteSurface::StaticAsset,
            RouteSurface::Ops,
            RouteSurface::Unknown,
        ] {
            assert_eq!(
                delta.record(FlowPageview { surface, ..page() }, Some(0)),
                None
            );
        }
        assert!(delta.is_empty());
    }
}
