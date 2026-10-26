//! Pure, bounded classification of registered HTTP routes.
//! Business authorization, anti-bot enforcement and recommendations are consumers,
//! not responsibilities of this crate.
#![forbid(unsafe_code)]

pub mod domain;

pub use domain::{
    attach_sql_evidence, classify, rank_route_costs, registry, route_cost_family, CostClass,
    CostMapError,
    LatencyBucket, ProductArea, RiskHint, RouteCostBucket, RouteCostRanking, RouteDecision,
    RouteSqlEvidence,
    RouteMatchKind, RouteObservation, RoutePassport, RouteRegistration, StatusBucket,
    TrafficSurface, COST_MAP_VERSION, COST_ROUTE_COUNT, RIL_CLASSIFIER_VERSION,
    RIL_TAXONOMY_VERSION, ROUTE_KNOWN_DEFINITION_VERSION,
};
