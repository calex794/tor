//! Pure, bounded classification of registered HTTP routes.
//! Business authorization, anti-bot enforcement and recommendations are consumers,
//! not responsibilities of this crate.
#![forbid(unsafe_code)]

pub mod domain;

pub use domain::{
    classify, registry, CostClass, LatencyBucket, ProductArea, RiskHint, RouteDecision,
    RouteMatchKind, RouteObservation, RoutePassport, RouteRegistration, StatusBucket,
    TrafficSurface, RIL_CLASSIFIER_VERSION, RIL_TAXONOMY_VERSION, ROUTE_KNOWN_DEFINITION_VERSION,
};
