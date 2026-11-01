//! A-09/referral routes added after the stable Layer and account IDs 0..=114.
use super::super::*;
use CostClass::{CheapRead as Read, Write};
use ProductArea::{Auth, Dashboard};
use RouteMatchKind::{Exact, Prefix};
use TrafficSurface::{Api, PublicPage as Page};

#[rustfmt::skip]
pub(super) const REFERRAL_ROUTES: [RouteRegistration; 9] = [
    RouteRegistration::new("/r/{code}", "GET", "/r/", Prefix, Page, Auth, Write), // 115
    RouteRegistration::new("/me/referrals", "GET", "/me/referrals/", Exact, Page, Dashboard, Read), // 116
    RouteRegistration::new("/me/referrals", "POST", "/me/referrals/", Exact, Page, Dashboard, Write), // 117
    RouteRegistration::new("/api/v1/me/referrals", "GET", "/api/v1/me/referrals/", Exact, Api, Dashboard, Read), // 118
    RouteRegistration::new("/api/v1/me/referrals", "POST", "/api/v1/me/referrals/", Exact, Api, Dashboard, Write), // 119
    RouteRegistration::new("/go/{alias}", "GET", "/go/", Prefix, Page, Auth, Read), // 120
    RouteRegistration::new("/promo/{alias}", "GET", "/promo/", Prefix, Page, Auth, Read), // 121
    RouteRegistration::new("/me/referrals/alias", "POST", "/me/referrals/alias/", Exact, Page, Dashboard, Write), // 122
    RouteRegistration::new("/api/v1/me/referrals/alias", "POST", "/api/v1/me/referrals/alias/", Exact, Api, Dashboard, Write), // 123
];
