//! Append-only E-03 canonical Inbox browser/JSON routes on reviewed IDs 94-95.
use super::super::*;
use CostClass::Read;
use ProductArea::Dashboard;
use RouteMatchKind::Exact;
use TrafficSurface::{Api, PublicPage as Page};

pub(super) const INBOX_ROUTES: [RouteRegistration; 2] = [
    RouteRegistration::new(
        "/me/inbox",
        "GET",
        "/me/inbox/",
        Exact,
        Page,
        Dashboard,
        Read,
    ), // 94
    RouteRegistration::new(
        "/api/v1/me/inbox",
        "GET",
        "/api/v1/me/inbox/",
        Exact,
        Api,
        Dashboard,
        Read,
    ), // 95
];
