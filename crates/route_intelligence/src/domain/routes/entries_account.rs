//! Latest account/profile/security routes, appended after D-05 IDs 0..=100.
use super::super::*;
use CostClass::{CheapRead as Read, Free, Write};
use ProductArea::{Auth, Dashboard, System};
use RouteMatchKind::Exact;
use TrafficSurface::{Api, PublicPage as Page, Static};

#[rustfmt::skip]
pub(super) const ACCOUNT_ROUTES: [RouteRegistration; 14] = [
    RouteRegistration::new("/me/settings", "GET", "/me/settings/", Exact, Page, Dashboard, Read), // 101
    RouteRegistration::new("/me/settings", "POST", "/me/settings/", Exact, Page, Dashboard, Write), // 102
    RouteRegistration::new("/api/v1/me", "GET", "/api/v1/me/", Exact, Api, Auth, Read), // 103
    RouteRegistration::new("/api/v1/me", "PATCH", "/api/v1/me/", Exact, Api, Auth, Write), // 104
    RouteRegistration::new("/me/notification-settings", "GET", "/me/notification-settings/", Exact, Page, Dashboard, Read), // 105
    RouteRegistration::new("/me/notification-settings", "POST", "/me/notification-settings/", Exact, Page, Dashboard, Write), // 106
    RouteRegistration::new("/forgot-password", "GET", "/forgot-password/", Exact, Page, Auth, Read), // 107
    RouteRegistration::new("/forgot-password", "POST", "/forgot-password/", Exact, Page, Auth, Write), // 108
    RouteRegistration::new("/reset-password", "GET", "/reset-password/", Exact, Page, Auth, Read), // 109
    RouteRegistration::new("/reset-password", "POST", "/reset-password/", Exact, Page, Auth, Write), // 110
    RouteRegistration::new("/api/v1/me/password", "POST", "/api/v1/me/password/", Exact, Api, Auth, Write), // 111
    RouteRegistration::new("/assets/account-reset.js", "GET", "/assets/", Exact, Static, System, Free), // 112
    RouteRegistration::new("/me/security/sessions", "GET", "/me/", Exact, Page, Auth, Read), // 113
    RouteRegistration::new("/me/security/sessions", "POST", "/me/", Exact, Page, Auth, Write), // 114
];
