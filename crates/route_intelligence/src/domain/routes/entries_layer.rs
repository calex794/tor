//! D-05 Layer routes follow C-15 continuation at 79..=87.
use super::super::*;
use CostClass::{CheapRead as Read, Free, Write};
use ProductArea::{Billing, Dashboard, Reading, System};
use RouteMatchKind::{Exact, Prefix};
use TrafficSurface::{PublicPage as Page, Static};
const L: &str = "/layers/";

#[rustfmt::skip]
pub(super) const LAYER_ROUTES: [RouteRegistration; 12] = [
    RouteRegistration::new(
        "/read/layer/{chapter_id}",
        "GET",
        "/read/",
        Prefix,
        Page,
        Reading,
        Read,
    ), // 79
    RouteRegistration::new(
        "/assets/newboo-layer-reader.mjs",
        "GET",
        "/assets/",
        Exact,
        Static,
        System,
        Free,
    ), // 80
    RouteRegistration::new(
        "/me/layers/{layer_id}/books",
        "GET",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Read,
    ), // 81
    RouteRegistration::new(
        "/me/layers/{layer_id}/books",
        "POST",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Write,
    ), // 82
    RouteRegistration::new("/me/layers/new", "GET", "/me/", Prefix, Page, Dashboard, Read),
    RouteRegistration::new("/me/layers/new", "POST", "/me/", Prefix, Page, Dashboard, Write),
    RouteRegistration::new("/layers/{layer_id}", "GET", L, Prefix, Page, Reading, Read),
    RouteRegistration::new("/layers/{layer_id}/buy", "POST", L, Prefix, Page, Reading, Write),
    RouteRegistration::new(
        "/me/layers/{layer_id}/offer",
        "POST",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Write,
    ), // 87
    RouteRegistration::new(
        "/api/v1/layers/{id}/purchase",
        "POST",
        "/api/v1/layers/",
        Prefix,
        TrafficSurface::Api,
        ProductArea::Billing,
        Write,
    ), // 88
    RouteRegistration::new(
        "/layers/{layer_id}/purchase",
        "POST",
        L,
        Prefix,
        Page,
        Billing,
        Write,
    ), // 89
    RouteRegistration::new(
        "/me/library/layers",
        "GET",
        "/me/library/",
        Exact,
        Page,
        Reading,
        Read,
    ), // 90
];
