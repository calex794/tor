//! Known MatchedPath taxonomy: fixed index, no per-request allocation.
use super::*;

// Indexes below are deliberately shared with classify(), and checked by unit tests.
// These templates reflect currently registered Axum routes, not future planned API.
use CostClass::{CheapRead as Read, Free, Write};
use ProductArea::{Auth, Catalog, Dashboard, Reading, System};
use RouteMatchKind::{Exact, Prefix};
use TrafficSurface::{Api, PublicPage as Page, Service, Static};

const ROUTES: [RouteRegistration; 60] = [
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
];

/// Read-only, finite route graph for subsequent governance checks.
pub const fn registry() -> &'static [RouteRegistration] {
    &ROUTES
}

/// Takes Axum's MatchedPath template, NEVER a raw request path.
/// The registry is indexed by a fixed match, not scanned per request.
/// HEAD is equivalent to GET for classification; HTTP method enforcement remains Axum's.
pub fn classify(matched_template: Option<&str>, method: &str) -> RoutePassport {
    let method = if method == "HEAD" { "GET" } else { method };
    let index = match (matched_template, method) {
        (Some("/"), "GET") => Some(0),
        (Some("/catalog"), "GET") => Some(1),
        (Some("/books/{id}"), "GET") => Some(2),
        (Some("/reader/{id}"), "GET") => Some(3),
        (Some("/assets/site.css"), "GET") => Some(4),
        (Some("/healthz"), "GET") => Some(5),
        (Some("/readyz"), "GET") => Some(6),
        (Some("/login"), "GET") => Some(7),
        (Some("/login"), "POST") => Some(8),
        (Some("/register"), "GET") => Some(9),
        (Some("/register"), "POST") => Some(10),
        (Some("/api/v1/books"), "GET") => Some(11),
        (Some("/me"), "GET") => Some(12),
        (Some("/me/library"), "GET") => Some(13),
        (Some("/read/{id}"), "GET") => Some(14),
        (Some("/read/{id}/place"), "POST") => Some(15),
        (Some("/logout"), "POST") => Some(16),
        (Some("/me/books"), "GET") => Some(17),
        (Some("/me/books"), "POST") => Some(18),
        (Some("/me/books/new"), "GET") => Some(19),
        (Some("/me/books/{id}"), "GET") => Some(20),
        (Some("/api/v1/books/{id}/toc"), "GET") => Some(21),
        (Some("/api/v1/me/library"), "GET") => Some(22),
        (Some("/api/v1/me/library/{id}"), "POST") => Some(23),
        (Some("/api/v1/me/library/{id}"), "DELETE") => Some(24),
        (Some("/me/library/add/{id}"), "GET") => Some(25),
        (Some("/me/library/{id}/save"), "POST") => Some(26),
        (Some("/me/library/{id}/remove"), "POST") => Some(27),
        (Some("/me/books/{id}/chapters/new"), "GET") => Some(28),
        (Some("/me/books/{id}/chapters"), "POST") => Some(29),
        (Some("/me/books/{id}/publish"), "POST") => Some(30),
        (Some("/api/v1/me/progress/free"), "GET") => Some(31),
        (Some("/api/v1/me/progress/free"), "PUT") => Some(32),
        (Some("/api/v1/me/progress/purchased"), "GET") => Some(33),
        (Some("/api/v1/me/progress/purchased"), "PUT") => Some(34),
        (Some("/me/books/{id}/chapters/{chapter_id}/edit"), "GET") => Some(35),
        (Some("/me/books/{id}/chapters/{chapter_id}/edit"), "POST") => Some(36),
        (Some("/me/behavior"), "POST") => Some(37),
        (Some("/me/books/{id}/chapters/{chapter_id}/release"), "GET") => Some(38),
        (Some("/me/books/{id}/chapters/{chapter_id}/release"), "POST") => Some(39),
        (Some("/api/v1/chapters/{id}/read"), "GET") => Some(40),
        (Some("/api/v1/search/books"), "GET") => Some(41),
        (Some("/assets/newboo.css"), "GET") => Some(42),
        (Some("/assets/brand/pandaolate-header.png"), "GET") => Some(43),
        (Some("/assets/brand/pandaolate-header-dark.png"), "GET") => Some(44),
        (Some("/metrics"), "GET") => Some(45),
        (Some("/ops/traces"), "GET") => Some(46),
        (Some("/ops/overview"), "GET") => Some(47),
        (Some("/covers/{id}/{variant}"), "GET") => Some(48),
        (Some("/me/rankings"), "GET") => Some(49),
        (Some("/me/rankings/choice"), "POST") => Some(50),
        (Some("/me/rankings/resume"), "POST") => Some(51),
        (Some("/api/v1/me/rankings"), "GET") => Some(52),
        (Some("/api/v1/me/rankings/choice"), "POST") => Some(53),
        (Some("/api/v1/me/rankings/resume"), "POST") => Some(54),
        (Some("/me/books/{id}/cover"), "POST") => Some(55),
        (Some("/me/books/{id}/settings/covers"), "GET") => Some(56),
        (Some("/assets/newboo-cover-picker.js"), "GET") => Some(57),
        (Some("/api/v1/purchases"), "POST") => Some(58),
        (Some("/api/v1/reader/chapters/{chapter_id}/delivery-grants"), "POST") => Some(59),
        _ => None,
    };
    index
        .map(|i| RoutePassport::from_route(&ROUTES[i]))
        .unwrap_or_else(RoutePassport::unknown)
}
