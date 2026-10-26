//! Process-local, cumulative and fixed-cardinality HTTP histograms.
//! No per-request records or user-associated data are retained.
#[cfg(unix)]
pub mod lookup;
#[cfg(unix)]
pub mod publisher;
pub mod process;
pub mod trace;
pub mod wire;

use crate::domain::{RouteGroup, StatusGroup, BOUNDS};
use std::fmt::Write;
use std::sync::Mutex;
use std::time::Duration;

#[derive(Clone, Copy, Default)]
struct Observation {
    count: u64,
    sum_ns: u128,
    buckets: [u64; BOUNDS.len()],
}

/// Anonymous cumulative process-local HTTP counts, reset on every restart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct HttpSummary {
    pub requests: u64,
    pub client_errors: u64,
    pub server_errors: u64,
    pub slower_than_one_second: u64,
}

#[derive(Default)]
pub struct HttpMetrics {
    observations: Mutex<[[Observation; StatusGroup::ALL.len()]; RouteGroup::ALL.len()]>,
}

impl HttpMetrics {
    /// Always counts completed requests; saturation never wraps an exported counter.
    pub fn observe(&self, route: RouteGroup, status: StatusGroup, duration: Duration) {
        let mut guard = self
            .observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let record = &mut guard[route as usize][status as usize];
        record.count = record.count.saturating_add(1);
        let elapsed = duration.as_nanos();
        record.sum_ns = record.sum_ns.saturating_add(elapsed);
        for (index, (bound, _)) in BOUNDS.iter().enumerate() {
            if elapsed <= *bound {
                record.buckets[index] = record.buckets[index].saturating_add(1);
            }
        }
    }

    /// A stable operator snapshot; never parses strings from /metrics.
    pub fn summary(&self) -> HttpSummary {
        let snapshot = *self
            .observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut total = HttpSummary::default();
        for groups in snapshot {
            for (status, counters) in groups.into_iter().enumerate() {
                total.requests = total.requests.saturating_add(counters.count);
                if status == StatusGroup::ClientError as usize {
                    total.client_errors = total.client_errors.saturating_add(counters.count);
                }
                if status == StatusGroup::ServerError as usize {
                    total.server_errors = total.server_errors.saturating_add(counters.count);
                }
                total.slower_than_one_second = total.slower_than_one_second.saturating_add(
                    counters.count.saturating_sub(counters.buckets[BOUNDS.len() - 1]),
                );
            }
        }
        total
    }

    /// This is not a distributed or persistent series. Process restarts reset it.
    pub fn prometheus(&self) -> String {
        let snapshot = *self
            .observations
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut out = String::from(
            "# HELP tor_http_request_duration_seconds HTTP handler duration (process-local).\n\
             # TYPE tor_http_request_duration_seconds histogram\n",
        );
        for (route_idx, groups) in snapshot.iter().enumerate() {
            let route = RouteGroup::ALL[route_idx].label();
            for (status_idx, bucket) in groups.iter().enumerate() {
                if bucket.count == 0 {
                    continue;
                }
                let status = StatusGroup::ALL[status_idx].label();
                for (idx, (_, bound_label)) in BOUNDS.iter().enumerate() {
                    writeln!(
                        out,
                        "tor_http_request_duration_seconds_bucket{{route_family=\"{route}\",status_group=\"{status}\",le=\"{bound_label}\"}} {}",
                        bucket.buckets[idx]
                    )
                    .expect("writing into String");
                }
                writeln!(
                    out,
                    "tor_http_request_duration_seconds_bucket{{route_family=\"{route}\",status_group=\"{status}\",le=\"+Inf\"}} {}",
                    bucket.count
                )
                .expect("writing into String");
                writeln!(
                    out,
                    "tor_http_request_duration_seconds_count{{route_family=\"{route}\",status_group=\"{status}\"}} {}",
                    bucket.count
                )
                .expect("writing into String");
                writeln!(
                    out,
                    "tor_http_request_duration_seconds_sum{{route_family=\"{route}\",status_group=\"{status}\"}} {}",
                    bucket.sum_ns as f64 / 1_000_000_000.0
                )
                .expect("writing into String");
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_and_buckets_are_cumulative_after_two_hundred_requests() {
        let metrics = HttpMetrics::default();
        for _ in 0..200 {
            metrics.observe(
                RouteGroup::Catalog,
                StatusGroup::Success,
                Duration::from_millis(15),
            );
        }
        let text = metrics.prometheus();
        assert!(text.contains(
            "tor_http_request_duration_seconds_count{route_family=\"catalog\",status_group=\"2xx\"} 200"
        ));
        assert!(text.contains(
            "tor_http_request_duration_seconds_bucket{route_family=\"catalog\",status_group=\"2xx\",le=\"0.01\"} 0"
        ));
        assert!(text.contains(
            "tor_http_request_duration_seconds_bucket{route_family=\"catalog\",status_group=\"2xx\",le=\"0.05\"} 200"
        ));
        assert!(text.contains(
            "tor_http_request_duration_seconds_bucket{route_family=\"catalog\",status_group=\"2xx\",le=\"+Inf\"} 200"
        ));
    }

    #[test]
    fn operator_snapshot_counts_errors_and_slow_without_identifiers() {
        let metrics = HttpMetrics::default();
        assert_eq!(metrics.summary(), HttpSummary::default());
        metrics.observe(RouteGroup::Home, StatusGroup::Success, Duration::from_millis(80));
        metrics.observe(RouteGroup::Reader, StatusGroup::ClientError, Duration::from_secs(2));
        metrics.observe(RouteGroup::Other, StatusGroup::ServerError, Duration::from_secs(4));
        let view = metrics.summary();
        assert_eq!(view.requests, 3);
        assert_eq!(view.client_errors, 1);
        assert_eq!(view.server_errors, 1);
        assert_eq!(view.slower_than_one_second, 2);
        assert!(!format!("{view:?}").contains("user_id"));
    }

    #[test]
    fn groups_are_independent_and_do_not_store_identity() {
        let metrics = HttpMetrics::default();
        metrics.observe(
            RouteGroup::Books,
            StatusGroup::ServerError,
            Duration::from_secs(2),
        );
        metrics.observe(RouteGroup::Other, StatusGroup::ClientError, Duration::ZERO);
        let text = metrics.prometheus();
        assert!(text.contains("route_family=\"books\",status_group=\"5xx\""));
        assert!(text.contains("route_family=\"other\",status_group=\"4xx\""));
        assert!(!text.contains("request_id"));
        assert!(!text.contains("user_id"));
        assert!(!text.contains("by_path"));
    }
}
