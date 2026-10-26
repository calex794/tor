//! I/G FinOps boundary: a cost report can only use finite, identical route codes.
use tor_route_intelligence::{
    rank_route_costs, route_cost_family, RouteCostBucket, COST_ROUTE_COUNT,
};
use tor_traffic_behavior::RouteCode;

#[test]
fn route_codes_in_pg_match_pure_route_intelligence_families() {
    let codes = [
        (RouteCode::Home, "/"),
        (RouteCode::Catalog, "/catalog/"),
        (RouteCode::Books, "/books/"),
        (RouteCode::Reader, "/reader/"),
        (RouteCode::Assets, "/assets/"),
        (RouteCode::Health, "/healthz/"),
        (RouteCode::Ready, "/readyz/"),
        (RouteCode::Login, "/login/"),
        (RouteCode::Register, "/register/"),
        (RouteCode::ApiBooks, "/api/v1/books/"),
        (RouteCode::Me, "/me/"),
        (RouteCode::Library, "/me/library/"),
        (RouteCode::Read, "/read/"),
        (RouteCode::Logout, "/logout/"),
        (RouteCode::Unknown, "/unknown/"),
    ];
    assert_eq!(codes.len(), COST_ROUTE_COUNT);
    for (code, expected) in codes {
        assert_eq!(code as i16, RouteCode::from_family(expected) as i16);
        assert_eq!(route_cost_family(code as i16), Some(expected));
    }
}

#[test]
fn rankings_never_provide_unmeasured_database_or_money_costs() {
    let ranked = rank_route_costs(
        &[
            RouteCostBucket {
                route_code: RouteCode::Books as i16,
                status_group: 1,
                latency_group: 2,
                request_count: 100,
            },
            RouteCostBucket {
                route_code: RouteCode::Ready as i16,
                status_group: 4,
                latency_group: 3,
                request_count: 3,
            },
        ],
        5,
    )
    .unwrap();
    assert_eq!(ranked.len(), 2);
    assert_eq!(ranked[0].route_family, "/books/");
    assert_eq!(ranked[0].requests, 100);
    assert_eq!(ranked[0].response_time_floor_ms, 10_000);
    for item in ranked {
        assert_eq!(item.db_queries, None);
        assert_eq!(item.db_time_ms, None);
        assert_eq!(item.monetary_cost, None);
        assert!(!item.route_family.contains("?"));
    }
}
