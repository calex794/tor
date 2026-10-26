//! Registered route templates are the ONLY input. Never feed raw URL paths or IDs.
//! The explicit match is constant-shape; no linear scan or allocated lookup per request.
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

// Indexes below are deliberately shared with classify(), and checked by unit tests.
// These templates reflect currently registered Axum routes, not future planned API.
use CostClass::{CheapRead as Read, Free, Write};
use ProductArea::{Auth, Catalog, Dashboard, Reading, System};
use RouteMatchKind::{Exact, Prefix};
use TrafficSurface::{Api, PublicPage as Page, Service, Static};

const ROUTES: [RouteRegistration; 29] = [
    RouteRegistration::new("/", "GET", "/", Exact, Page, Catalog, Read), // 0
    RouteRegistration::new("/catalog", "GET", "/catalog/", Exact, Page, Catalog, Read), // 1
    RouteRegistration::new("/books/{id}", "GET", "/books/", Prefix, Page, Reading, Read), // 2
    RouteRegistration::new(
        "/reader/{id}",
        "GET",
        "/reader/",
        Prefix,
        Page,
        Reading,
        Read,
    ), // 3
    RouteRegistration::new(
        "/assets/site.css",
        "GET",
        "/assets/",
        Exact,
        Static,
        System,
        Free,
    ), // 4
    RouteRegistration::new("/healthz", "GET", "/healthz/", Exact, Service, System, Free), // 5
    RouteRegistration::new("/readyz", "GET", "/readyz/", Exact, Service, System, Read), // 6
    RouteRegistration::new("/login", "GET", "/login/", Exact, Page, Auth, Read), // 7
    RouteRegistration::new("/login", "POST", "/login/", Exact, Page, Auth, Write), // 8
    RouteRegistration::new("/register", "GET", "/register/", Exact, Page, Auth, Read), // 9
    RouteRegistration::new("/register", "POST", "/register/", Exact, Page, Auth, Write), // 10
    RouteRegistration::new(
        "/api/v1/books",
        "GET",
        "/api/v1/books/",
        Exact,
        Api,
        Catalog,
        Read,
    ), // 11
    RouteRegistration::new("/me", "GET", "/me/", Exact, Page, Dashboard, Read), // 12
    RouteRegistration::new(
        "/me/library",
        "GET",
        "/me/library/",
        Exact,
        Page,
        Reading,
        Read,
    ), // 13
    RouteRegistration::new("/read/{id}", "GET", "/read/", Prefix, Page, Reading, Read), // 14
    RouteRegistration::new(
        "/read/{id}/place",
        "POST",
        "/read/",
        Prefix,
        Page,
        Reading,
        Write,
    ), // 15
    RouteRegistration::new("/logout", "POST", "/logout/", Exact, Page, Auth, Write), // 16
    RouteRegistration::new("/me/books", "GET", "/me/", Exact, Page, Dashboard, Read), // 17
    RouteRegistration::new("/me/books", "POST", "/me/", Exact, Page, Dashboard, Write), // 18
    RouteRegistration::new("/me/books/new", "GET", "/me/", Exact, Page, Dashboard, Read), // 19
    RouteRegistration::new(
        "/me/books/{id}",
        "GET",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Read,
    ), // 20
    RouteRegistration::new(
        "/api/v1/books/{id}/toc",
        "GET",
        "/api/v1/books/",
        Prefix,
        Api,
        Reading,
        Read,
    ), // 21
    RouteRegistration::new(
        "/api/v1/me/library",
        "GET",
        "/api/v1/me/library/",
        Exact,
        Api,
        Reading,
        Read,
    ), // 22
    RouteRegistration::new(
        "/api/v1/me/library/{id}",
        "POST",
        "/api/v1/me/library/",
        Prefix,
        Api,
        Reading,
        Write,
    ), // 23
    RouteRegistration::new(
        "/api/v1/me/library/{id}",
        "DELETE",
        "/api/v1/me/library/",
        Prefix,
        Api,
        Reading,
        Write,
    ), // 24
    RouteRegistration::new(
        "/me/library/add/{id}",
        "GET",
        "/me/library/",
        Prefix,
        Page,
        Reading,
        Read,
    ), // 25
    RouteRegistration::new(
        "/me/library/{id}/save",
        "POST",
        "/me/library/",
        Prefix,
        Page,
        Reading,
        Write,
    ), // 26
    RouteRegistration::new(
        "/me/library/{id}/remove",
        "POST",
        "/me/library/",
        Prefix,
        Page,
        Reading,
        Write,
    ), // 27
    RouteRegistration::new("/me/behavior", "POST", "/me/", Exact, Page, Dashboard, Write), // 28
];

/// Read-only, finite route graph for subsequent governance checks.
pub const fn registry() -> &'static [RouteRegistration] {
    &ROUTES
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

/// Takes Axum's MatchedPath template, NEVER a raw request path.
/// The registry is indexed by a fixed match, not scanned per request.
/// HEAD is equivalent to GET for classification; HTTP method enforcement remains Axum's.
pub fn classify(matched_template: Option<&str>, method: &str) -> RoutePassport {
    let method = if method == "HEAD" { "GET" } else { method };
    let index = match (matched_template, method) {
        (Some("/"), "GET") => Some(0),
        (Some("/catalog"), "GET") => Some(1),
        (Some("/books/{id}"), "GET") => Some(2),
        (Some("/reader/{id}"), "GET") => Some(3),
        (Some("/assets/site.css"), "GET") => Some(4),
        (Some("/healthz"), "GET") => Some(5),
        (Some("/readyz"), "GET") => Some(6),
        (Some("/login"), "GET") => Some(7),
        (Some("/login"), "POST") => Some(8),
        (Some("/register"), "GET") => Some(9),
        (Some("/register"), "POST") => Some(10),
        (Some("/api/v1/books"), "GET") => Some(11),
        (Some("/me"), "GET") => Some(12),
        (Some("/me/library"), "GET") => Some(13),
        (Some("/read/{id}"), "GET") => Some(14),
        (Some("/read/{id}/place"), "POST") => Some(15),
        (Some("/logout"), "POST") => Some(16),
        (Some("/me/books"), "GET") => Some(17),
        (Some("/me/books"), "POST") => Some(18),
        (Some("/me/books/new"), "GET") => Some(19),
        (Some("/me/books/{id}"), "GET") => Some(20),
        (Some("/api/v1/books/{id}/toc"), "GET") => Some(21),
        (Some("/api/v1/me/library"), "GET") => Some(22),
        (Some("/api/v1/me/library/{id}"), "POST") => Some(23),
        (Some("/api/v1/me/library/{id}"), "DELETE") => Some(24),
        (Some("/me/library/add/{id}"), "GET") => Some(25),
        (Some("/me/library/{id}/save"), "POST") => Some(26),
        (Some("/me/library/{id}/remove"), "POST") => Some(27),
        (Some("/me/behavior"), "POST") => Some(28),
        _ => None,
    };
    index
        .map(|i| RoutePassport::from_route(&ROUTES[i]))
        .unwrap_or_else(RoutePassport::unknown)
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
