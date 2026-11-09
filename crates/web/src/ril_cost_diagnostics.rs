//! Read-only RIL diagnostic aggregation outside the HTTP router module.
use crate::PgCatalog;

pub async fn ril_cost_map_for_diagnostics(
    catalog: &PgCatalog,
    days: u16,
    limit: usize,
) -> Option<Vec<tor_route_intelligence::RouteCostRanking>> {
    use tor_route_intelligence::{rank_route_costs, RouteCostBucket};
    use tor_traffic_behavior::repositories::PgRilRollups;

    let rows = PgRilRollups::new(catalog.pool())
        .read_cost_buckets(days)
        .await
        .ok()?;
    let buckets: Vec<RouteCostBucket> = rows
        .into_iter()
        .map(|row| RouteCostBucket {
            route_code: row.route,
            status_group: row.status,
            latency_group: row.latency,
            request_count: row.count,
        })
        .collect();
    // Read SQLx evidence independently; if missing, keep the latency map
    // but leave query fields explicitly unmeasured.
    let mut ranked = rank_route_costs(&buckets, limit).ok()?;
    if let Ok(measured) = PgRilRollups::new(catalog.pool()).read_sql_costs(days).await {
        let evidence: Vec<tor_route_intelligence::RouteSqlEvidence> = measured
            .into_iter()
            .map(|row| tor_route_intelligence::RouteSqlEvidence {
                route_code: row.route,
                observed_requests: row.observed_requests,
                observed_queries: row.observed_queries,
                sqlx_elapsed_us: row.sqlx_elapsed_us,
            })
            .collect();
        if tor_route_intelligence::attach_sql_evidence(&mut ranked, &evidence).is_err() {
            return None; // Inconsistent PG evidence: no misleading report.
        }
    }
    Some(ranked)
}
