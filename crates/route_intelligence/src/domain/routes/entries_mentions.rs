//! E-03 comment mentions, appended after stable D-05 + F-09 IDs 0..=123.
use super::super::*;
use CostClass::CheapRead as Read;
use ProductArea::{Dashboard, System};
use RouteMatchKind::Exact;
use TrafficSurface::{Api, Static};

#[rustfmt::skip]
pub(super) const MENTION_ROUTES: [RouteRegistration; 2] = [
    RouteRegistration::new("/assets/mentions.js", "GET", "/assets/", Exact, Static, System, Read), // 124
    RouteRegistration::new("/api/v1/social/mentions/discover", "GET", "/api/v1/social/", Exact, Api, Dashboard, Read), // 125
];
