//! Registered route templates are the ONLY input. Never feed raw URL paths or IDs.
//! The explicit match is constant-shape; no linear scan or allocated lookup per request.
mod cost_map;
mod routes;
pub use cost_map::{
    attach_sql_evidence, rank_route_costs, route_cost_family, CostMapError, RouteCostBucket,
    RouteCostRanking, RouteSqlEvidence, COST_MAP_VERSION, COST_ROUTE_COUNT,
};
pub use routes::{classify, registry};

use std::time::Duration;

pub const RIL_TAXONOMY_VERSION: &str = "ril_taxonomy_v1";
pub const RIL_CLASSIFIER_VERSION: &str = "ril_classifier_v1";
pub const ROUTE_KNOWN_DEFINITION_VERSION: &str = "route_known_v1";

macro_rules! bounded {
    ($name:ident { $($variant:ident => $label:literal),+ $(,)? }) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name { $($variant),+ }
        impl $name {
            pub const fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $label),+ }
            }
        }
    };
}

bounded!(RouteMatchKind {
    Exact => "exact", Prefix => "prefix", Fallback => "fallback", Unknown => "unknown"
});
bounded!(RouteDecision {
    Allow => "allow", Diagnostics => "diagnostics", Allowlisted => "allowlisted", Deny => "deny"
});
bounded!(TrafficSurface {
    PublicPage => "public_page", Api => "api", Internal => "internal",
    Admin => "admin", Static => "static", Service => "service", Unknown => "unknown"
});
bounded!(ProductArea {
    Reading => "reading", Catalog => "catalog", Auth => "auth", Billing => "billing",
    Admin => "admin", Advertising => "advertising", Dashboard => "dashboard",
    Social => "social", Mail => "mail", Notifications => "notifications",
    Recommendations => "recommendations", Marketplace => "marketplace",
    Storage => "storage", System => "system", Unknown => "unknown"
});
bounded!(CostClass {
    Free => "free", CheapRead => "cheap_read", ExpensiveRead => "expensive_read",
    Write => "write", Background => "background", Unknown => "unknown"
});
bounded!(RiskHint {
    Normal => "normal", Suspicious => "suspicious", Scanner => "scanner", Unknown => "unknown"
});
bounded!(StatusBucket {
    Information => "1xx", Success => "2xx", Redirect => "3xx",
    ClientError => "4xx", ServerError => "5xx", Unknown => "unknown"
});
bounded!(LatencyBucket {
    Under10Ms => "lt_10ms", Under100Ms => "lt_100ms",
    Under1Sec => "lt_1s", Slow => "gte_1s"
});

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteRegistration {
    pub template: &'static str,
    pub method: &'static str,
    pub family: &'static str,
    pub match_kind: RouteMatchKind,
    pub surface: TrafficSurface,
    pub area: ProductArea,
    pub cost: CostClass,
    pub risk: RiskHint,
}

/// Checkout route counted in the static taxonomy audit of domain/mod.rs.
pub(super) const LAYER_CHECKOUT_ROUTE: RouteRegistration = RouteRegistration::new(
    "/api/v1/layers/{id}/purchase",
    "POST",
    "/api/v1/layers/",
    RouteMatchKind::Prefix,
    TrafficSurface::Api,
    ProductArea::Billing,
    CostClass::Write,
);

pub(super) const LAYER_CREATE_PAGE_ROUTE: RouteRegistration = RouteRegistration::new(
    "/me/layers/new",
    "GET",
    "/me/",
    RouteMatchKind::Prefix,
    TrafficSurface::PublicPage,
    ProductArea::Dashboard,
    CostClass::CheapRead,
);

pub(super) const LAYER_STOREFRONT_PAGE_ROUTE: RouteRegistration = RouteRegistration::new(
    "/layers/{id}",
    "GET",
    "/layers/",
    RouteMatchKind::Prefix,
    TrafficSurface::PublicPage,
    ProductArea::Billing,
    CostClass::CheapRead,
);

pub(super) const LAYER_STOREFRONT_BUY_ROUTE: RouteRegistration = RouteRegistration::new(
    "/layers/{id}/purchase",
    "POST",
    "/layers/",
    RouteMatchKind::Prefix,
    TrafficSurface::PublicPage,
    ProductArea::Billing,
    CostClass::Write,
);

pub(super) const LAYER_PURCHASED_TERMS_ROUTE: RouteRegistration = RouteRegistration::new(
    "/me/library/layers",
    "GET",
    "/me/library/",
    RouteMatchKind::Exact,
    TrafficSurface::PublicPage,
    ProductArea::Reading,
    CostClass::CheapRead,
);

impl RouteRegistration {
    const fn new(
        template: &'static str,
        method: &'static str,
        family: &'static str,
        match_kind: RouteMatchKind,
        surface: TrafficSurface,
        area: ProductArea,
        cost: CostClass,
    ) -> Self {
        Self {
            template,
            method,
            family,
            match_kind,
            surface,
            area,
            cost,
            risk: RiskHint::Normal,
        }
    }
}

/// This is diagnostic metadata, not a substitute for authorization or route policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RoutePassport {
    pub route_known: bool,
    pub route_family: &'static str,
    pub route_match_kind: RouteMatchKind,
    pub route_decision: RouteDecision,
    pub traffic_surface: TrafficSurface,
    pub product_area: ProductArea,
    pub cost_class: CostClass,
    pub risk_hint: RiskHint,
    pub ril_taxonomy_version: &'static str,
    pub ril_classifier_version: &'static str,
    pub route_known_definition_version: &'static str,
}

impl RoutePassport {
    const fn from_route(route: &RouteRegistration) -> Self {
        Self {
            route_known: true,
            route_family: route.family,
            route_match_kind: route.match_kind,
            route_decision: RouteDecision::Allow,
            traffic_surface: route.surface,
            product_area: route.area,
            cost_class: route.cost,
            risk_hint: route.risk,
            ril_taxonomy_version: RIL_TAXONOMY_VERSION,
            ril_classifier_version: RIL_CLASSIFIER_VERSION,
            route_known_definition_version: ROUTE_KNOWN_DEFINITION_VERSION,
        }
    }

    const fn unknown() -> Self {
        Self {
            route_known: false,
            route_family: "/unknown/",
            route_match_kind: RouteMatchKind::Unknown,
            route_decision: RouteDecision::Diagnostics,
            traffic_surface: TrafficSurface::Unknown,
            product_area: ProductArea::Unknown,
            cost_class: CostClass::Unknown,
            risk_hint: RiskHint::Unknown,
            ril_taxonomy_version: RIL_TAXONOMY_VERSION,
            ril_classifier_version: RIL_CLASSIFIER_VERSION,
            route_known_definition_version: ROUTE_KNOWN_DEFINITION_VERSION,
        }
    }
}

/// Fixed-cardinality, non-identifying counters only; no raw URL, user, IP or query.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RouteObservation {
    pub passport: RoutePassport,
    pub status_bucket: StatusBucket,
    pub latency_bucket: LatencyBucket,
}

impl RouteObservation {
    pub fn new(passport: RoutePassport, status: u16, elapsed: Duration) -> Self {
        let status_bucket = match status / 100 {
            1 => StatusBucket::Information,
            2 => StatusBucket::Success,
            3 => StatusBucket::Redirect,
            4 => StatusBucket::ClientError,
            5 => StatusBucket::ServerError,
            _ => StatusBucket::Unknown,
        };
        let latency_bucket = if elapsed.as_millis() < 10 {
            LatencyBucket::Under10Ms
        } else if elapsed.as_millis() < 100 {
            LatencyBucket::Under100Ms
        } else if elapsed.as_millis() < 1000 {
            LatencyBucket::Under1Sec
        } else {
            LatencyBucket::Slow
        };
        Self {
            passport,
            status_bucket,
            latency_bucket,
        }
    }
}

#[cfg(test)]
mod tests;
