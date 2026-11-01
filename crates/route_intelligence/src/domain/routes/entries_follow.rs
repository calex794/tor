//! Append-only C-15 book follow routes; preserve Layer/S035 route IDs.
use super::super::*;
use CostClass::Write;
use ProductArea::Reading;
use RouteMatchKind::Prefix;
use TrafficSurface::PublicPage as Page;

pub(super) const FOLLOW_ROUTES: [RouteRegistration; 2] = [
    RouteRegistration::new(
        "/me/books/{id}/follow",
        "POST",
        "/me/",
        Prefix,
        Page,
        Reading,
        Write,
    ), // 91
    RouteRegistration::new(
        "/me/books/{id}/unfollow",
        "POST",
        "/me/",
        Prefix,
        Page,
        Reading,
        Write,
    ), // 92
];
