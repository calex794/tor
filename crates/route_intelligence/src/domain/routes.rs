use super::*;
mod classifier;
pub use classifier::classify;

use CostClass::{CheapRead as Read, Free, Write};
use ProductArea::{Auth, Catalog, Dashboard, Reading, System};
use RouteMatchKind::{Exact, Prefix};
use TrafficSurface::{Api, PublicPage as Page, Service, Static};

// Fixed-index route registry: keep compact entries stable for review.
#[rustfmt::skip]
const ROUTES: [RouteRegistration; 103] = [
    RouteRegistration::new("/", "GET", "/", Exact, Page, Catalog, Read), // 0
    RouteRegistration::new("/catalog", "GET", "/catalog/", Exact, Page, Catalog, Read), // 1
    RouteRegistration::new("/books/{id}", "GET", "/books/", Prefix, Page, Reading, Read), // 2
    RouteRegistration::new("/reader/{id}", "GET", "/reader/", Prefix, Page, Reading, Read), // 3
    RouteRegistration::new("/assets/site.css", "GET", "/assets/", Exact, Static, System, Free), // 4
    RouteRegistration::new("/healthz", "GET", "/healthz/", Exact, Service, System, Free), // 5
    RouteRegistration::new("/readyz", "GET", "/readyz/", Exact, Service, System, Read), // 6
    RouteRegistration::new("/login", "GET", "/login/", Exact, Page, Auth, Read), // 7
    RouteRegistration::new("/login", "POST", "/login/", Exact, Page, Auth, Write), // 8
    RouteRegistration::new("/register", "GET", "/register/", Exact, Page, Auth, Read), // 9
    RouteRegistration::new("/register", "POST", "/register/", Exact, Page, Auth, Write), // 10
    RouteRegistration::new("/api/v1/books", "GET", "/api/v1/books/", Exact, Api, Catalog, Read), // 11
    RouteRegistration::new("/me", "GET", "/me/", Exact, Page, Dashboard, Read), // 12
    RouteRegistration::new("/me/library", "GET", "/me/library/", Exact, Page, Reading, Read), // 13
    RouteRegistration::new("/read/{id}", "GET", "/read/", Prefix, Page, Reading, Read), // 14
    RouteRegistration::new("/read/{id}/place", "POST", "/read/", Prefix, Page, Reading, Write), // 15
    RouteRegistration::new("/logout", "POST", "/logout/", Exact, Page, Auth, Write), // 16
    RouteRegistration::new("/me/books", "GET", "/me/", Exact, Page, Dashboard, Read), // 17
    RouteRegistration::new("/me/books", "POST", "/me/", Exact, Page, Dashboard, Write), // 18
    RouteRegistration::new("/me/books/new", "GET", "/me/", Exact, Page, Dashboard, Read), // 19
    RouteRegistration::new("/me/books/{id}", "GET", "/me/", Prefix, Page, Dashboard, Read), // 20
    RouteRegistration::new("/api/v1/books/{id}/toc", "GET", "/api/v1/books/", Prefix, Api, Reading, Read), // 21
    RouteRegistration::new("/api/v1/me/library", "GET", "/api/v1/me/library/", Exact, Api, Reading, Read), // 22
    RouteRegistration::new("/api/v1/me/library/{id}", "POST", "/api/v1/me/library/", Prefix, Api, Reading, Write), // 23
    RouteRegistration::new("/api/v1/me/library/{id}", "DELETE", "/api/v1/me/library/", Prefix, Api, Reading, Write), // 24
    RouteRegistration::new("/me/library/add/{id}", "GET", "/me/library/", Prefix, Page, Reading, Read), // 25
    RouteRegistration::new("/me/library/{id}/save", "POST", "/me/library/", Prefix, Page, Reading, Write), // 26
    RouteRegistration::new("/me/library/{id}/remove", "POST", "/me/library/", Prefix, Page, Reading, Write), // 27
    RouteRegistration::new("/me/books/{id}/chapters/new", "GET", "/me/", Prefix, Page, Dashboard, Read), // 28
    RouteRegistration::new("/me/books/{id}/chapters", "POST", "/me/", Prefix, Page, Dashboard, Write), // 29
    RouteRegistration::new("/me/books/{id}/publish", "POST", "/me/", Prefix, Page, Dashboard, Write), // 30
    RouteRegistration::new("/api/v1/me/progress/free", "GET", "/api/v1/me/progress/", Exact, Api, Reading, Read), // 31
    RouteRegistration::new("/api/v1/me/progress/free", "PUT", "/api/v1/me/progress/", Exact, Api, Reading, Write), // 32
    RouteRegistration::new("/api/v1/me/progress/purchased", "GET", "/api/v1/me/progress/", Exact, Api, Reading, Read), // 33
    RouteRegistration::new("/api/v1/me/progress/purchased", "PUT", "/api/v1/me/progress/", Exact, Api, Reading, Write), // 34
    RouteRegistration::new("/me/books/{id}/chapters/{chapter_id}/edit", "GET", "/me/", Prefix, Page, Dashboard, Read), // 35
    RouteRegistration::new("/me/books/{id}/chapters/{chapter_id}/edit", "POST", "/me/", Prefix, Page, Dashboard, Write), // 36
    RouteRegistration::new("/me/behavior", "POST", "/me/", Exact, Page, Dashboard, Write), // 37
    RouteRegistration::new("/me/books/{id}/chapters/{chapter_id}/release", "GET", "/me/", Prefix, Page, Dashboard, Read), // 38
    RouteRegistration::new("/me/books/{id}/chapters/{chapter_id}/release", "POST", "/me/", Prefix, Page, Dashboard, Write), // 39
    RouteRegistration::new("/api/v1/chapters/{id}/read", "GET", "/api/v1/chapters/", Prefix, Api, Reading, Read), // 40
    RouteRegistration::new("/api/v1/search/books", "GET", "/api/v1/books/", Exact, Api, Catalog, Read), // 41
    RouteRegistration::new("/assets/newboo.css", "GET", "/assets/", Exact, Static, System, Free), // 42
    RouteRegistration::new("/assets/brand/pandaolate-header.png", "GET", "/assets/", Exact, Static, System, Free), // 43
    RouteRegistration::new("/assets/brand/pandaolate-header-dark.png", "GET", "/assets/", Exact, Static, System, Free), // 44
    RouteRegistration::new("/metrics", "GET", "/healthz/", Exact, Service, System, Read), // 45
    RouteRegistration::new("/ops/traces", "GET", "/ops/", Exact, TrafficSurface::Admin, ProductArea::Admin, Read), // 46
    RouteRegistration::new("/ops/overview", "GET", "/ops/", Exact, TrafficSurface::Admin, ProductArea::Admin, Read), // 47
    RouteRegistration::new("/covers/{id}/{variant}", "GET", "/assets/", Prefix, Api, ProductArea::Storage, CostClass::ExpensiveRead), // 48
    RouteRegistration::new("/me/rankings", "GET", "/me/", Exact, Page, ProductArea::Recommendations, Read), // 49
    RouteRegistration::new("/me/rankings/choice", "POST", "/me/", Exact, Page, ProductArea::Recommendations, Write), // 50
    RouteRegistration::new("/me/rankings/resume", "POST", "/me/", Exact, Page, ProductArea::Recommendations, Write), // 51
    RouteRegistration::new("/api/v1/me/rankings", "GET", "/api/v1/me/rankings/", Exact, Api, ProductArea::Recommendations, Read), // 52
    RouteRegistration::new("/api/v1/me/rankings/choice", "POST", "/api/v1/me/rankings/", Exact, Api, ProductArea::Recommendations, Write), // 53
    RouteRegistration::new("/api/v1/me/rankings/resume", "POST", "/api/v1/me/rankings/", Exact, Api, ProductArea::Recommendations, Write), // 54
    RouteRegistration::new("/me/books/{id}/cover", "POST", "/me/", Prefix, Page, Dashboard, Write), // 55
    RouteRegistration::new("/me/books/{id}/settings/covers", "GET", "/me/", Prefix, Page, Dashboard, Read), // 56
    RouteRegistration::new("/assets/newboo-cover-picker.js", "GET", "/assets/", Exact, Static, System, Free), // 57
    RouteRegistration::new("/api/v1/purchases", "POST", "/api/v1/purchases/", Exact, Api, ProductArea::Billing, Write), // 58
    RouteRegistration::new("/api/v1/reader/chapters/{chapter_id}/delivery-grants", "POST", "/api/v1/reader/chapters/", Prefix, Api, Reading, CostClass::ExpensiveRead), // 59
    RouteRegistration::new("/read/{id}/complete", "POST", "/read/", Prefix, Page, Reading, Write), // 60
    RouteRegistration::new("/read/paid/{chapter_id}", "GET", "/read/", Prefix, Page, Reading, Read), // 61
    RouteRegistration::new("/assets/newboo-paid-reader.mjs", "GET", "/assets/", Exact, Static, System, Free), // 62
    RouteRegistration::new("/me/purchases", "GET", "/me/library/", Exact, Page, Reading, Read), // 63
    RouteRegistration::new("/api/v1/me/progress/purchased/complete", "GET", "/api/v1/me/progress/", Exact, Api, Reading, Read), // 64
    RouteRegistration::new("/api/v1/me/progress/purchased/complete", "POST", "/api/v1/me/progress/", Exact, Api, Reading, Write), // 65
    RouteRegistration::new("/me/books/{id}/rate", "GET", "/me/", Prefix, Page, Reading, Read), // 66
    RouteRegistration::new("/me/books/{id}/rate", "POST", "/me/", Prefix, Page, Reading, Write), // 67
    RouteRegistration::new("/me/books/{id}/description", "GET", "/me/", Prefix, Page, Dashboard, Read), // 68
    RouteRegistration::new("/me/books/{id}/description", "POST", "/me/", Prefix, Page, Dashboard, Write), // 69
    RouteRegistration::new("/api/v1/reader/chapters/{chapter_id}/assets/{object_id}/delivery-grants", "POST", "/api/v1/reader/chapters/", Prefix, Api, Reading, CostClass::ExpensiveRead), // 70
    RouteRegistration::new("/books/{id}/comments", "GET", "/books/", Prefix, Page, Reading, Read), // 71
    RouteRegistration::new("/books/{id}/comments", "POST", "/books/", Prefix, Page, Reading, Write), // 72
    RouteRegistration::new("/me/books/{id}/chapters/illustrated", "GET", "/me/", Prefix, Page, Dashboard, Read), // 73
    RouteRegistration::new("/me/books/{id}/chapters/illustrated", "POST", "/me/", Prefix, Page, Dashboard, Write), // 74
    RouteRegistration::new("/api/v1/purchases/batch", "POST", "/api/v1/purchases/", Exact, Api, ProductArea::Billing, Write), // 75
    RouteRegistration::new("/me/books/{id}/buy-chapters", "GET", "/me/", Prefix, Page, ProductArea::Billing, Read), // 76
    RouteRegistration::new("/me/books/{id}/buy-chapters", "POST", "/me/", Prefix, Page, ProductArea::Billing, Write), // 77
    RouteRegistration::new("/read/{id}/want-continuation", "POST", "/read/", Prefix, Page, Reading, Write), // 78
    RouteRegistration::new("/me/books/{id}/follow", "POST", "/me/", Prefix, Page, Reading, Write), // 79
    RouteRegistration::new("/me/books/{id}/unfollow", "POST", "/me/", Prefix, Page, Reading, Write), // 80
    RouteRegistration::new("/me/replies/{id}/read-state", "POST", "/me/replies/", Prefix, Page, Dashboard, Write), // 81
    RouteRegistration::new("/me/inbox", "GET", "/me/inbox/", Exact, Page, Dashboard, Read), // 82
    RouteRegistration::new("/api/v1/me/inbox", "GET", "/api/v1/me/inbox/", Exact, Api, Dashboard, Read), // 83
    RouteRegistration::new("/me/updates/{id}/read-state", "POST", "/me/updates/", Prefix, Page, Reading, Write), // 84
    RouteRegistration::new("/me/settings/password", "GET", "/me/", Exact, Page, Auth, Read), // 85
    RouteRegistration::new("/me/settings/password", "POST", "/me/", Exact, Page, Auth, Write), // 86
    RouteRegistration::new("/me/settings", "GET", "/me/settings/", Exact, Page, Dashboard, Read), // 87
    RouteRegistration::new("/me/settings", "POST", "/me/settings/", Exact, Page, Dashboard, Write), // 88
    RouteRegistration::new("/api/v1/me", "GET", "/api/v1/me/", Exact, Api, Auth, Read), // 89
    RouteRegistration::new("/api/v1/me", "PATCH", "/api/v1/me/", Exact, Api, Auth, Write), // 90
    RouteRegistration::new("/me/notification-settings", "GET", "/me/notification-settings/", Exact, Page, Dashboard, Read), // 91
    RouteRegistration::new("/me/notification-settings", "POST", "/me/notification-settings/", Exact, Page, Dashboard, Write), // 92
    RouteRegistration::new("/forgot-password", "GET", "/forgot-password/", Exact, Page, Auth, Read), // 93
    RouteRegistration::new("/forgot-password", "POST", "/forgot-password/", Exact, Page, Auth, Write), // 94
    RouteRegistration::new("/reset-password", "GET", "/reset-password/", Exact, Page, Auth, Read), // 95
    RouteRegistration::new("/reset-password", "POST", "/reset-password/", Exact, Page, Auth, Write), // 96
    RouteRegistration::new("/api/v1/me/password", "POST", "/api/v1/me/password/", Exact, Api, Auth, Write), // 97
    RouteRegistration::new("/assets/account-reset.js", "GET", "/assets/", Exact, Static, System, Free), // 98
    RouteRegistration::new("/me/security/sessions", "GET", "/me/", Exact, Page, Auth, Read), // 99
    RouteRegistration::new("/me/security/sessions", "POST", "/me/", Exact, Page, Auth, Write), // 100
    RouteRegistration::new("/books/{id}/stats", "GET", "/books/", Prefix, Page, Dashboard, Read), // 101
    RouteRegistration::new("/books/{id}/stats/export.csv", "GET", "/books/", Prefix, Page, Dashboard, Read), // 102
];

pub const fn registry() -> &'static [RouteRegistration] {
    &ROUTES
}
