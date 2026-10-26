//! RIL-10: bounded, read-only FinOps evidence from coarse RIL-06 histograms.
//! Not billing, not CPU usage, and not measured SQL counts or money.
//! Only finite route codes: never full paths, book IDs, user IDs or queries.

pub const COST_MAP_VERSION: &str = "ril_cost_map_v1";
pub const COST_ROUTE_COUNT: usize = 15;
const STATUS_COUNT: usize = 6;
const LATENCY_COUNT: usize = 4;
const MAX_BUCKETS: usize = COST_ROUTE_COUNT * STATUS_COUNT * LATENCY_COUNT;
const LOWER_MS: [u64; LATENCY_COUNT] = [0, 10, 100, 1000];
const UPPER_MS_EXCLUSIVE: [Option<u64>; LATENCY_COUNT] = [Some(10), Some(100), Some(1000), None];

// The code order is a contract with G's fixed RouteCode and RIL-06 migration.
// Change together and update the cross-feature bridge tests.
const FAMILIES: [&str; COST_ROUTE_COUNT] = [
    "/",
    "/catalog/",
    "/books/",
    "/reader/",
    "/assets/",
    "/healthz/",
    "/readyz/",
    "/login/",
    "/register/",
    "/api/v1/books/",
    "/me/",
    "/me/library/",
    "/read/",
    "/logout/",
    "/unknown/",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteCostBucket {
    pub route_code: i16,
    pub status_group: i16,
    pub latency_group: i16,
    pub request_count: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CostMapError {
    InvalidBucket,
    DuplicateBucket,
    TooManyBuckets,
    InvalidSqlEvidence,
    DuplicateSqlEvidence,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteCostRanking {
    pub route_code: i16,
    pub route_family: &'static str,
    pub requests: u64,
    pub client_errors: u64,
    pub server_errors: u64,
    pub slow_requests: u64,
    /// Conservative minimum for the sum of request durations, not actual time.
    pub response_time_floor_ms: u64,
    /// Observed histogram interval, not an exact p95.
    pub p95_lower_ms: u64,
    pub p95_upper_exclusive_ms: Option<u64>,
    /// Requests with observed in-task SQLx completion events (even 0 queries).
    /// Some(0) means measured zero; None means no measurement at all.
    pub sqlx_observed_requests: Option<u64>,
    /// Client-observed SQLx query completions, only for the measured requests.
    pub sqlx_observed_queries: Option<u64>,
    /// SQLx client-side elapsed microseconds, not PostgreSQL server CPU time.
    pub sqlx_elapsed_us: Option<u64>,
    /// Total actual PG query count remains unknown without full-coverage proof.
    pub db_queries: Option<u64>,
    pub db_time_ms: Option<u64>,
    /// No measured CPU, resource pricing or billing rate is available.
    pub monetary_cost: Option<u64>,
}

#[derive(Clone, Copy, Default)]
struct Totals {
    requests: u64,
    client_errors: u64,
    server_errors: u64,
    slow_requests: u64,
    response_time_floor_ms: u64,
    histogram: [u64; LATENCY_COUNT],
}

pub fn route_cost_family(route_code: i16) -> Option<&'static str> {
    let route = usize::try_from(route_code).ok()?;
    FAMILIES.get(route).copied()
}

/// Stable top-N ranking by *minimum possible* total observed response time.
/// No per-request SQL, no unbounded map, no guessed query count or currency.
/// The final tie-break uses the finite route code, not arbitrary user labels.
pub fn rank_route_costs(
    rows: &[RouteCostBucket],
    limit: usize,
) -> Result<Vec<RouteCostRanking>, CostMapError> {
    if rows.len() > MAX_BUCKETS {
        return Err(CostMapError::TooManyBuckets);
    }
    let mut totals = [Totals::default(); COST_ROUTE_COUNT];
    let mut seen = [false; MAX_BUCKETS];
    for row in rows {
        let (route, status, latency) = (
            usize::try_from(row.route_code).map_err(|_| CostMapError::InvalidBucket)?,
            usize::try_from(row.status_group).map_err(|_| CostMapError::InvalidBucket)?,
            usize::try_from(row.latency_group).map_err(|_| CostMapError::InvalidBucket)?,
        );
        if route >= COST_ROUTE_COUNT
            || status >= STATUS_COUNT
            || latency >= LATENCY_COUNT
            || row.request_count <= 0
        {
            return Err(CostMapError::InvalidBucket);
        }
        let index = (route * STATUS_COUNT + status) * LATENCY_COUNT + latency;
        if seen[index] {
            return Err(CostMapError::DuplicateBucket);
        }
        seen[index] = true;
        let count = row.request_count as u64;
        let value = &mut totals[route];
        value.requests = value.requests.saturating_add(count);
        value.histogram[latency] = value.histogram[latency].saturating_add(count);
        value.response_time_floor_ms = value
            .response_time_floor_ms
            .saturating_add(count.saturating_mul(LOWER_MS[latency]));
        if status == 3 {
            value.client_errors = value.client_errors.saturating_add(count);
        } else if status == 4 {
            value.server_errors = value.server_errors.saturating_add(count);
        }
        if latency == 3 {
            value.slow_requests = value.slow_requests.saturating_add(count);
        }
    }
    let mut ranked: Vec<_> = totals
        .iter()
        .enumerate()
        .filter(|(_, total)| total.requests > 0)
        .map(|(route, total)| {
            let target = (total.requests as u128 * 95).div_ceil(100) as u64;
            let mut collected = 0_u64;
            let mut p95 = 3;
            for (bucket, count) in total.histogram.iter().enumerate() {
                collected = collected.saturating_add(*count);
                if collected >= target {
                    p95 = bucket;
                    break;
                }
            }
            RouteCostRanking {
                route_code: route as i16,
                route_family: FAMILIES[route],
                requests: total.requests,
                client_errors: total.client_errors,
                server_errors: total.server_errors,
                slow_requests: total.slow_requests,
                response_time_floor_ms: total.response_time_floor_ms,
                p95_lower_ms: LOWER_MS[p95],
                p95_upper_exclusive_ms: UPPER_MS_EXCLUSIVE[p95],
                sqlx_observed_requests: None,
                sqlx_observed_queries: None,
                sqlx_elapsed_us: None,
                db_queries: None,
                db_time_ms: None,
                monetary_cost: None,
            }
        })
        .collect();
    ranked.sort_by(|a, b| {
        b.response_time_floor_ms
            .cmp(&a.response_time_floor_ms)
            .then_with(|| b.server_errors.cmp(&a.server_errors))
            .then_with(|| b.client_errors.cmp(&a.client_errors))
            .then_with(|| b.requests.cmp(&a.requests))
            .then_with(|| a.route_code.cmp(&b.route_code))
    });
    ranked.truncate(limit.min(COST_ROUTE_COUNT));
    Ok(ranked)
}

/// Measurements cover only SQLx completions within observed request tasks.
/// Detached tasks, dropped queue entries, and non-SQLx work are NOT counted.
/// Fail rather than invent a total when sampling exceeds the HTTP tally.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteSqlEvidence {
    pub route_code: i16,
    pub observed_requests: i64,
    pub observed_queries: i64,
    pub sqlx_elapsed_us: i64,
}

pub fn attach_sql_evidence(
    rankings: &mut [RouteCostRanking],
    evidence: &[RouteSqlEvidence],
) -> Result<(), CostMapError> {
    if evidence.len() > COST_ROUTE_COUNT {
        return Err(CostMapError::InvalidSqlEvidence);
    }
    let mut seen = [false; COST_ROUTE_COUNT];
    for record in evidence {
        let route =
            usize::try_from(record.route_code).map_err(|_| CostMapError::InvalidSqlEvidence)?;
        if route >= COST_ROUTE_COUNT
            || record.observed_requests <= 0
            || record.observed_queries < 0
            || record.sqlx_elapsed_us < 0
        {
            return Err(CostMapError::InvalidSqlEvidence);
        }
        if seen[route] {
            return Err(CostMapError::DuplicateSqlEvidence);
        }
        seen[route] = true;
        if let Some(row) = rankings
            .iter_mut()
            .find(|row| row.route_code == record.route_code)
        {
            if record.observed_requests as u64 > row.requests {
                return Err(CostMapError::InvalidSqlEvidence);
            }
            row.sqlx_observed_requests = Some(record.observed_requests as u64);
            row.sqlx_observed_queries = Some(record.observed_queries as u64);
            row.sqlx_elapsed_us = Some(record.sqlx_elapsed_us as u64);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bucket(route: i16, status: i16, latency: i16, count: i64) -> RouteCostBucket {
        RouteCostBucket {
            route_code: route,
            status_group: status,
            latency_group: latency,
            request_count: count,
        }
    }

    #[test]
    fn orders_by_observed_response_time_floor_not_raw_request_volume() {
        let costs = rank_route_costs(
            &[
                bucket(1, 1, 0, 20_000),
                bucket(2, 4, 3, 100),
                bucket(3, 1, 2, 300),
            ],
            10,
        )
        .unwrap();
        assert_eq!(
            costs.iter().map(|r| r.route_code).collect::<Vec<_>>(),
            vec![2, 3, 1]
        );
        assert_eq!(costs[0].response_time_floor_ms, 100_000);
        assert_eq!(costs[0].server_errors, 100);
        assert_eq!(costs[0].p95_lower_ms, 1000);
        assert_eq!(costs[0].p95_upper_exclusive_ms, None);
    }

    #[test]
    fn p95_histogram_bucket_is_honest_about_uncertainty() {
        let result = rank_route_costs(
            &[bucket(9, 1, 0, 94), bucket(9, 1, 1, 2), bucket(9, 1, 2, 4)],
            15,
        )
        .unwrap();
        assert_eq!(result[0].p95_lower_ms, 10);
        assert_eq!(result[0].p95_upper_exclusive_ms, Some(100));
        assert_eq!(result[0].requests, 100);
        assert_eq!(result[0].response_time_floor_ms, 420);
        assert_eq!(result[0].db_queries, None);
        assert_eq!(result[0].db_time_ms, None);
        assert_eq!(result[0].monetary_cost, None);
    }

    #[test]
    fn all_status_groups_and_exact_route_labels_are_bounded() {
        let result = rank_route_costs(
            &[bucket(14, 3, 3, 5), bucket(14, 4, 3, 2), bucket(0, 1, 0, 6)],
            2,
        )
        .unwrap();
        assert_eq!(result[0].route_family, "/unknown/");
        assert_eq!(result[0].client_errors, 5);
        assert_eq!(result[0].server_errors, 2);
        assert_eq!(result[0].slow_requests, 7);
        assert_eq!(route_cost_family(-1), None);
        assert_eq!(route_cost_family(15), None);
    }

    #[test]
    fn invalid_negative_and_duplicate_groups_fail_closed() {
        for bad in [
            bucket(-1, 1, 1, 1),
            bucket(15, 1, 1, 1),
            bucket(0, -1, 1, 1),
            bucket(0, 6, 1, 1),
            bucket(0, 1, 4, 1),
            bucket(0, 1, 1, 0),
            bucket(0, 1, 1, -2),
        ] {
            assert_eq!(
                rank_route_costs(&[bad], 4),
                Err(CostMapError::InvalidBucket)
            );
        }
        assert_eq!(
            rank_route_costs(&[bucket(1, 1, 1, 3), bucket(1, 1, 1, 2)], 2),
            Err(CostMapError::DuplicateBucket)
        );
    }

    #[test]
    fn bounded_output_stable_ties_and_empty_input() {
        assert_eq!(rank_route_costs(&[], 10).unwrap().len(), 0);
        assert!(rank_route_costs(&[bucket(9, 1, 1, 3)], 0)
            .unwrap()
            .is_empty());
        let a = rank_route_costs(&[bucket(2, 1, 1, 10), bucket(1, 1, 1, 10)], 15).unwrap();
        let b = rank_route_costs(&[bucket(1, 1, 1, 10), bucket(2, 1, 1, 10)], 15).unwrap();
        assert_eq!(a, b);
        assert_eq!(a[0].route_code, 1);
    }

    #[test]
    fn no_unbounded_allocation_or_integer_overflow_from_counts() {
        let all = (0..15)
            .flat_map(|route| {
                (0..6).flat_map(move |status| {
                    (0..4).map(move |latency| bucket(route, status, latency, i64::MAX))
                })
            })
            .collect::<Vec<_>>();
        let top = rank_route_costs(&all, usize::MAX).unwrap();
        assert_eq!(top.len(), 15);
        assert_eq!(top[0].requests, u64::MAX);
        assert_eq!(top[0].response_time_floor_ms, u64::MAX);
        assert_eq!(
            rank_route_costs(&[all[0]; MAX_BUCKETS + 1], 1),
            Err(CostMapError::TooManyBuckets)
        );
    }

    #[test]
    fn measured_sql_evidence_has_explicit_coverage_and_never_invents_total_cost() {
        let mut rows = rank_route_costs(&[bucket(2, 1, 1, 12)], 15).unwrap();
        assert_eq!(rows[0].sqlx_observed_requests, None);
        attach_sql_evidence(
            &mut rows,
            &[RouteSqlEvidence {
                route_code: 2,
                observed_requests: 6,
                observed_queries: 9,
                sqlx_elapsed_us: 4500,
            }],
        )
        .unwrap();
        assert_eq!(rows[0].sqlx_observed_requests, Some(6));
        assert_eq!(rows[0].sqlx_observed_queries, Some(9));
        assert_eq!(rows[0].sqlx_elapsed_us, Some(4500));
        assert_eq!(rows[0].db_queries, None);
        assert_eq!(rows[0].db_time_ms, None);
        assert_eq!(rows[0].monetary_cost, None);
    }

    #[test]
    fn missing_evidence_is_not_measured_zero_and_bad_coverage_fails_closed() {
        let mut rows = rank_route_costs(&[bucket(2, 1, 0, 2)], 15).unwrap();
        attach_sql_evidence(&mut rows, &[]).unwrap();
        assert_eq!(rows[0].sqlx_observed_queries, None);
        let zero = RouteSqlEvidence {
            route_code: 2,
            observed_requests: 1,
            observed_queries: 0,
            sqlx_elapsed_us: 0,
        };
        attach_sql_evidence(&mut rows, &[zero]).unwrap();
        assert_eq!(rows[0].sqlx_observed_queries, Some(0));
        assert_eq!(
            attach_sql_evidence(&mut rows, &[zero, zero]),
            Err(CostMapError::DuplicateSqlEvidence),
        );
        assert_eq!(
            attach_sql_evidence(
                &mut rows,
                &[RouteSqlEvidence {
                    observed_requests: 3,
                    ..zero
                }]
            ),
            Err(CostMapError::InvalidSqlEvidence),
        );
    }
}
