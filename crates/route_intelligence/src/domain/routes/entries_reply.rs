//! E-03 actor-bound reply read-state action; route 93 is append-only.
use super::super::*;
use CostClass::Write;
use ProductArea::Dashboard;
use RouteMatchKind::Prefix;
use TrafficSurface::PublicPage as Page;

pub(super) const REPLY_ROUTES: [RouteRegistration; 1] = [
    RouteRegistration::new(
        "/me/replies/{id}/read-state",
        "POST",
        "/me/replies/",
        Prefix,
        Page,
        Dashboard,
        Write,
    ), // 93
];
