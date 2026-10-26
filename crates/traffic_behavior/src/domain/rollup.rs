//! Fixed-size, in-process hourly deltas. No raw request data or identities.
//! Each dimension is an enum value, never a free-form label.
use super::{BehaviorEvent, GapBucket, RouteCode};

const ROUTES: usize = 15;
const STATUS: usize = 6;
const LATENCY: usize = 4;
const GAPS: usize = 4;
const MAX_CELL: u64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transition {
    pub from: RouteCode,
    pub to: RouteCode,
    pub gap: GapBucket,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteCount {
    pub route: i16,
    pub status: i16,
    pub latency: i16,
    pub count: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransitionCount {
    pub from: i16,
    pub to: i16,
    pub gap: i16,
    pub count: i64,
}

/// A bounded batch is ~10 KiB of fixed-size counters, regardless of traffic.
/// This aggregation is independent of the long-lived per-subject BehaviorSketch.
#[derive(Clone)]
pub struct RollupDelta {
    routes: [[[u64; LATENCY]; STATUS]; ROUTES],
    transitions: [[[u64; GAPS]; ROUTES]; ROUTES],
    capped_cells: u64,
}

impl Default for RollupDelta {
    fn default() -> Self {
        Self {
            routes: [[[0; LATENCY]; STATUS]; ROUTES],
            transitions: [[[0; GAPS]; ROUTES]; ROUTES],
            capped_cells: 0,
        }
    }
}

impl RollupDelta {
    pub fn record(&mut self, event: BehaviorEvent, transition: Option<Transition>) {
        let cell =
            &mut self.routes[event.route as usize][event.status as usize][event.latency as usize];
        if *cell < MAX_CELL {
            *cell += 1;
        } else {
            self.capped_cells = self.capped_cells.saturating_add(1);
        }
        if let Some(transition) = transition {
            let cell = &mut self.transitions[transition.from as usize][transition.to as usize]
                [transition.gap as usize];
            if *cell < MAX_CELL {
                *cell += 1;
            } else {
                self.capped_cells = self.capped_cells.saturating_add(1);
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.routes.iter().flatten().flatten().all(|n| *n == 0)
            && self.transitions.iter().flatten().flatten().all(|n| *n == 0)
    }

    pub fn capped_cells(&self) -> u64 {
        self.capped_cells
    }

    pub fn route_rows(&self) -> Vec<RouteCount> {
        let mut output = Vec::new();
        for (route, statuses) in self.routes.iter().enumerate() {
            for (status, latencies) in statuses.iter().enumerate() {
                for (latency, count) in latencies.iter().enumerate() {
                    if *count > 0 {
                        output.push(RouteCount {
                            route: route as i16,
                            status: status as i16,
                            latency: latency as i16,
                            count: *count as i64,
                        });
                    }
                }
            }
        }
        output
    }

    pub fn transition_rows(&self) -> Vec<TransitionCount> {
        let mut output = Vec::new();
        for (from, destinations) in self.transitions.iter().enumerate() {
            for (to, gaps) in destinations.iter().enumerate() {
                for (gap, count) in gaps.iter().enumerate() {
                    if *count > 0 {
                        output.push(TransitionCount {
                            from: from as i16,
                            to: to as i16,
                            gap: gap as i16,
                            count: *count as i64,
                        });
                    }
                }
            }
        }
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{LatencyGroup, StatusGroup};
    use std::time::Duration;

    #[test]
    fn bounded_sparse_cells_preserve_status_and_latency_per_route() {
        let mut delta = RollupDelta::default();
        assert!(delta.is_empty());
        for (route, status) in [(RouteCode::Catalog, 200), (RouteCode::Reader, 500)] {
            delta.record(
                BehaviorEvent {
                    route,
                    status: StatusGroup::from_status(status),
                    latency: LatencyGroup::from_elapsed(Duration::from_secs(2)),
                    subject: None,
                    at_millis: 1,
                },
                None,
            );
        }
        assert!(!delta.is_empty());
        assert_eq!(delta.route_rows().len(), 2);
        assert!(delta
            .route_rows()
            .iter()
            .any(|row| row.route == RouteCode::Reader as i16
                && row.status == StatusGroup::ServerError as i16
                && row.latency == LatencyGroup::Slow as i16
                && row.count == 1));
        assert!(delta.transition_rows().is_empty());
    }

    #[test]
    fn transition_dimensions_do_not_expose_subjects() {
        let mut delta = RollupDelta::default();
        delta.record(
            BehaviorEvent {
                route: RouteCode::Books,
                status: StatusGroup::Success,
                latency: LatencyGroup::Under10Ms,
                subject: None,
                at_millis: 12,
            },
            Some(Transition {
                from: RouteCode::Catalog,
                to: RouteCode::Books,
                gap: GapBucket::Fast,
            }),
        );
        assert_eq!(
            delta.transition_rows(),
            vec![TransitionCount {
                from: RouteCode::Catalog as i16,
                to: RouteCode::Books as i16,
                gap: GapBucket::Fast as i16,
                count: 1,
            }]
        );
    }
}
