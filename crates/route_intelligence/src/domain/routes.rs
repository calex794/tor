//! Known MatchedPath taxonomy: fixed index, no per-request allocation.
use super::*;

mod classifier;
pub use classifier::classify;

// Indexes below are deliberately shared with classify(), and checked by unit tests.
// These templates reflect currently registered Axum routes, not future planned API.
use CostClass::{CheapRead as Read, Free, Write};
use ProductArea::{Auth, Catalog, Dashboard, Reading, System};
use RouteMatchKind::{Exact, Prefix};
use TrafficSurface::{Api, PublicPage as Page, Service, Static};

const ROUTES: [RouteRegistration; 76] = [
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
    RouteRegistration::new(
        "/me/books/{id}/chapters/new",
        "GET",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Read,
    ), // 28
    RouteRegistration::new(
        "/me/books/{id}/chapters",
        "POST",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Write,
    ), // 29
    RouteRegistration::new(
        "/me/books/{id}/publish",
        "POST",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Write,
    ), // 30
    RouteRegistration::new(
        "/api/v1/me/progress/free",
        "GET",
        "/api/v1/me/progress/",
        Exact,
        Api,
        Reading,
        Read,
    ), // 31
    RouteRegistration::new(
        "/api/v1/me/progress/free",
        "PUT",
        "/api/v1/me/progress/",
        Exact,
        Api,
        Reading,
        Write,
    ), // 32
    RouteRegistration::new(
        "/api/v1/me/progress/purchased",
        "GET",
        "/api/v1/me/progress/",
        Exact,
        Api,
        Reading,
        Read,
    ), // 33
    RouteRegistration::new(
        "/api/v1/me/progress/purchased",
        "PUT",
        "/api/v1/me/progress/",
        Exact,
        Api,
        Reading,
        Write,
    ), // 34
    RouteRegistration::new(
        "/me/books/{id}/chapters/{chapter_id}/edit",
        "GET",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Read,
    ), // 35
    RouteRegistration::new(
        "/me/books/{id}/chapters/{chapter_id}/edit",
        "POST",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Write,
    ), // 36
    RouteRegistration::new(
        "/me/behavior",
        "POST",
        "/me/",
        Exact,
        Page,
        Dashboard,
        Write,
    ), // 37
    RouteRegistration::new(
        "/me/books/{id}/chapters/{chapter_id}/release",
        "GET",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Read,
    ), // 38
    RouteRegistration::new(
        "/me/books/{id}/chapters/{chapter_id}/release",
        "POST",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Write,
    ), // 39
    RouteRegistration::new(
        "/api/v1/chapters/{id}/read",
        "GET",
        "/api/v1/chapters/",
        Prefix,
        Api,
        Reading,
        Read,
    ), // 40
    RouteRegistration::new(
        "/api/v1/search/books",
        "GET",
        "/api/v1/books/",
        Exact,
        Api,
        Catalog,
        Read,
    ), // 41
    RouteRegistration::new(
        "/assets/newboo.css",
        "GET",
        "/assets/",
        Exact,
        Static,
        System,
        Free,
    ), // 42
    RouteRegistration::new(
        "/assets/brand/pandaolate-header.png",
        "GET",
        "/assets/",
        Exact,
        Static,
        System,
        Free,
    ), // 43
    RouteRegistration::new(
        "/assets/brand/pandaolate-header-dark.png",
        "GET",
        "/assets/",
        Exact,
        Static,
        System,
        Free,
    ), // 44
    // Internal protected diagnostics: reuse finite technical-service family.
    RouteRegistration::new("/metrics", "GET", "/healthz/", Exact, Service, System, Read), // 45
    // Protected operator-only diagnostics are separate from public site accounts.
    RouteRegistration::new(
        "/ops/traces",
        "GET",
        "/ops/",
        Exact,
        TrafficSurface::Admin,
        ProductArea::Admin,
        Read,
    ), // 46
    RouteRegistration::new(
        "/ops/overview",
        "GET",
        "/ops/",
        Exact,
        TrafficSurface::Admin,
        ProductArea::Admin,
        Read,
    ), // 47
    // Private object read: not a free static image; bounded DB + S3 I/O.
    RouteRegistration::new(
        "/covers/{id}/{variant}",
        "GET",
        "/assets/",
        Prefix,
        Api,
        ProductArea::Storage,
        CostClass::ExpensiveRead,
    ), // 48
    RouteRegistration::new(
        "/me/rankings",
        "GET",
        "/me/",
        Exact,
        Page,
        ProductArea::Recommendations,
        Read,
    ), // 49
    RouteRegistration::new(
        "/me/rankings/choice",
        "POST",
        "/me/",
        Exact,
        Page,
        ProductArea::Recommendations,
        Write,
    ), // 50
    RouteRegistration::new(
        "/me/rankings/resume",
        "POST",
        "/me/",
        Exact,
        Page,
        ProductArea::Recommendations,
        Write,
    ), // 51
    RouteRegistration::new(
        "/api/v1/me/rankings",
        "GET",
        "/api/v1/me/rankings/",
        Exact,
        Api,
        ProductArea::Recommendations,
        Read,
    ), // 52
    RouteRegistration::new(
        "/api/v1/me/rankings/choice",
        "POST",
        "/api/v1/me/rankings/",
        Exact,
        Api,
        ProductArea::Recommendations,
        Write,
    ), // 53
    RouteRegistration::new(
        "/api/v1/me/rankings/resume",
        "POST",
        "/api/v1/me/rankings/",
        Exact,
        Api,
        ProductArea::Recommendations,
        Write,
    ), // 54
    RouteRegistration::new(
        "/me/books/{id}/cover",
        "POST",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Write,
    ), // 55
    RouteRegistration::new(
        "/me/books/{id}/settings/covers",
        "GET",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Read,
    ), // 56
    RouteRegistration::new(
        "/assets/newboo-cover-picker.js",
        "GET",
        "/assets/",
        Exact,
        Static,
        System,
        Free,
    ), // 57
    RouteRegistration::new(
        "/api/v1/purchases",
        "POST",
        "/api/v1/purchases/",
        Exact,
        Api,
        ProductArea::Billing,
        Write,
    ), // 58
    // Signed, scoped paid content grant. Performs PG rights checks and S3 HEAD.
    RouteRegistration::new(
        "/api/v1/reader/chapters/{chapter_id}/delivery-grants",
        "POST",
        "/api/v1/reader/chapters/",
        Prefix,
        Api,
        Reading,
        CostClass::ExpensiveRead,
    ), // 59
    RouteRegistration::new(
        "/read/{id}/complete",
        "POST",
        "/read/",
        Prefix,
        Page,
        Reading,
        Write,
    ), // 60
    RouteRegistration::new(
        "/read/paid/{chapter_id}",
        "GET",
        "/read/",
        Prefix,
        Page,
        Reading,
        Read,
    ), // 61
    RouteRegistration::new(
        "/assets/newboo-paid-reader.mjs",
        "GET",
        "/assets/",
        Exact,
        Static,
        System,
        Free,
    ), // 62
    // Only an authenticated, entitled actor can enumerate owned purchases.
    RouteRegistration::new(
        "/me/purchases",
        "GET",
        "/me/library/",
        Exact,
        Page,
        Reading,
        Read,
    ), // 63
    RouteRegistration::new(
        "/api/v1/me/progress/purchased/complete",
        "GET",
        "/api/v1/me/progress/",
        Exact,
        Api,
        Reading,
        Read,
    ), // 64
    RouteRegistration::new(
        "/api/v1/me/progress/purchased/complete",
        "POST",
        "/api/v1/me/progress/",
        Exact,
        Api,
        Reading,
        Write,
    ), // 65
    RouteRegistration::new(
        "/me/books/{id}/rate",
        "GET",
        "/me/",
        Prefix,
        Page,
        Reading,
        Read,
    ), // 66
    RouteRegistration::new(
        "/me/books/{id}/rate",
        "POST",
        "/me/",
        Prefix,
        Page,
        Reading,
        Write,
    ), // 67
    RouteRegistration::new(
        "/me/books/{id}/description",
        "GET",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Read,
    ), // 68
    RouteRegistration::new(
        "/me/books/{id}/description",
        "POST",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Write,
    ), // 69
    RouteRegistration::new(
        "/api/v1/reader/chapters/{chapter_id}/assets/{object_id}/delivery-grants",
        "POST",
        "/api/v1/reader/chapters/",
        Prefix,
        Api,
        Reading,
        CostClass::ExpensiveRead,
    ), // 70
    RouteRegistration::new(
        "/books/{id}/comments",
        "GET",
        "/books/",
        Prefix,
        Page,
        Reading,
        Read,
    ), // 71
    RouteRegistration::new(
        "/books/{id}/comments",
        "POST",
        "/books/",
        Prefix,
        Page,
        Reading,
        Write,
    ), // 72
    RouteRegistration::new(
        "/me/books/{id}/chapters/illustrated",
        "GET",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Read,
    ), // 73
    RouteRegistration::new(
        "/me/books/{id}/chapters/illustrated",
        "POST",
        "/me/",
        Prefix,
        Page,
        Dashboard,
        Write,
    ), // 74
    // D/S035: strict authenticated batch checkout from the internal wallet.
    RouteRegistration::new(
        "/api/v1/purchases/batch",
        "POST",
        "/api/v1/purchases/",
        Exact,
        Api,
        ProductArea::Billing,
        Write,
    ), // 75
];

/// Read-only, finite route graph for subsequent governance checks.
pub const fn registry() -> &'static [RouteRegistration] {
    &ROUTES
}
