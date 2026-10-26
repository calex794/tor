//! Fixed, bounded labels for technical HTTP measurements.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub enum RouteGroup {
    Home,
    Catalog,
    Books,
    Reader,
    Other,
}

impl RouteGroup {
    pub const ALL: [Self; 5] = [
        Self::Home,
        Self::Catalog,
        Self::Books,
        Self::Reader,
        Self::Other,
    ];

    /// Only accepts RIL's finite, trusted family. Never pass a raw URL here.
    pub fn from_ril_family(family: &str) -> Self {
        match family {
            "/" => Self::Home,
            "/catalog/" => Self::Catalog,
            "/books/" | "/api/v1/books/" => Self::Books,
            "/reader/" | "/read/" => Self::Reader,
            _ => Self::Other,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Catalog => "catalog",
            Self::Books => "books",
            Self::Reader => "reader",
            Self::Other => "other",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(usize)]
pub enum StatusGroup {
    Success,
    Redirect,
    ClientError,
    ServerError,
    Other,
}

impl StatusGroup {
    pub const ALL: [Self; 5] = [
        Self::Success,
        Self::Redirect,
        Self::ClientError,
        Self::ServerError,
        Self::Other,
    ];

    pub const fn from_http(status: u16) -> Self {
        match status {
            200..=299 => Self::Success,
            300..=399 => Self::Redirect,
            400..=499 => Self::ClientError,
            500..=599 => Self::ServerError,
            _ => Self::Other,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Success => "2xx",
            Self::Redirect => "3xx",
            Self::ClientError => "4xx",
            Self::ServerError => "5xx",
            Self::Other => "other",
        }
    }
}

/// Cumulative Prometheus buckets in nanoseconds, not rolling quantiles.
pub const BOUNDS: [(u128, &str); 6] = [
    (5_000_000, "0.005"),
    (10_000_000, "0.01"),
    (50_000_000, "0.05"),
    (100_000_000, "0.1"),
    (500_000_000, "0.5"),
    (1_000_000_000, "1"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_finite_and_never_echo_paths() {
        assert_eq!(RouteGroup::from_ril_family("/catalog/"), RouteGroup::Catalog);
        assert_eq!(RouteGroup::from_ril_family("/catalog/123?token=SECRET"), RouteGroup::Other);
        assert_eq!(RouteGroup::Other.label(), "other");
        assert_eq!(StatusGroup::from_http(503), StatusGroup::ServerError);
    }
}
