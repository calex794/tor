//! AB-12: cheap, bounded scanner-path rejection in the currently deployed TOR binary.
//! This is NOT an IP blocklist, WAF or bot identification model. It has zero
//! dynamic keys, untrusted proxy headers, database calls or event allocations.
use axum::{
    body::Body,
    http::{header, HeaderName, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Router,
};
use std::sync::atomic::{AtomicU64, Ordering};

static REJECTED: AtomicU64 = AtomicU64::new(0);

pub fn rejected_total() -> u64 {
    REJECTED.load(Ordering::Relaxed)
}

/// Applied at the outermost HTTP router, after all optional routes are merged.
/// A scanner probe is stopped before auth, SQLx, RIL and page rendering.
pub fn protect(router: Router) -> Router {
    router.layer(middleware::from_fn(reject_scanners))
}

/// Only deny well-known *non-TOR* attack surfaces. Ordinary unknown URLs
/// retain the existing 404; crawlers, readers and legitimate archives are
/// never rejected by user-agent, missing cookies or an IP/NAT heuristic.
fn scanner_path(path: &str) -> bool {
    let mut segments = path.split('/').filter(|value| !value.is_empty());
    let Some(first) = segments.next() else {
        return false;
    };
    let is_root_probe = [
        "wp-admin",
        "wp-content",
        "wp-includes",
        "wp-login.php",
        "xmlrpc.php",
        "phpmyadmin",
        "pma",
        "cgi-bin",
        "actuator",
        "server-status",
    ]
    .iter()
    .any(|candidate| first.eq_ignore_ascii_case(candidate));
    if is_root_probe {
        return true;
    }
    if first.eq_ignore_ascii_case("vendor")
        && segments
            .next()
            .is_some_and(|second| second.eq_ignore_ascii_case("phpunit"))
    {
        return true;
    }
    // Any directory level containing server secrets or repository metadata,
    // or a PHP script, is never a legitimate TOR route.
    path.split('/')
        .filter(|value| !value.is_empty())
        .any(|segment| {
            [".env", ".git", ".svn", ".hg", ".htaccess", ".htpasswd"]
                .iter()
                .any(|candidate| segment.eq_ignore_ascii_case(candidate))
                || segment
                    .get(segment.len().saturating_sub(4)..)
                    .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".php"))
        })
}

async fn reject_scanners(request: Request<Body>, next: Next) -> Response {
    if !scanner_path(request.uri().path()) {
        return next.run(request).await;
    }
    REJECTED.fetch_add(1, Ordering::Relaxed);
    (
        StatusCode::NOT_FOUND,
        [
            (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
            (
                HeaderName::from_static("x-robots-tag"),
                HeaderValue::from_static("noindex, nofollow"),
            ),
        ],
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use std::sync::{atomic::AtomicUsize, Arc};
    use tower::ServiceExt;

    #[test]
    fn only_known_scan_paths_are_rejected() {
        for bad in [
            "/wp-admin",
            "/wp-admin/",
            "//wp-content/plugins/hello",
            "/WP-LOGIN.PHP",
            "/xmlrpc.php",
            "/PHPMYADMIN/index.php",
            "/pma",
            "/cgi-bin/test",
            "/vendor/phpunit/src/Utils.php",
            "/.env",
            "/.git/config",
            "/static/.htaccess",
            "/foo/Shell.PHP",
            "/actuator/health",
            "/server-status",
        ] {
            assert!(scanner_path(bad), "should reject {bad}");
        }
        for allowed in [
            "/",
            "/catalog",
            "/books/42",
            "/reader/9",
            "/me/library",
            "/api/v1/chapters/22/read",
            "/assets/site.css",
            "/robots.txt",
            "/sitemap.xml",
            "/.well-known/acme-challenge/abc",
            "/downloads/chapter.zip",
            "/users/wp-admin",
            "/login",
            "/register",
            "/assets/brand/logo.png",
            "/search?q=phpunit",
        ] {
            assert!(!scanner_path(allowed), "must not reject {allowed}");
        }
    }

    #[tokio::test]
    async fn scanner_probe_never_reaches_downstream_handlers() {
        let downstream = Arc::new(AtomicUsize::new(0));
        let visits = downstream.clone();
        let app = protect(Router::new().fallback(move || {
            let visits = visits.clone();
            async move {
                visits.fetch_add(1, Ordering::Relaxed);
                StatusCode::NO_CONTENT
            }
        }));
        let before = rejected_total();
        for uri in ["/wp-admin/?key=SECRET", "/.git/config", "/cgi-bin/ping"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("POST")
                        .uri(uri)
                        .header("x-forwarded-for", "203.0.113.11")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
            assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
            assert!(response.headers().get("x-robots-tag").is_some());
            assert!(to_bytes(response.into_body(), 64).await.unwrap().is_empty());
        }
        assert!(rejected_total() >= before + 3);
        assert_eq!(downstream.load(Ordering::Relaxed), 0);
        let legitimate = app
            .oneshot(
                Request::builder()
                    .uri("/reader/9?token=SECRET")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(legitimate.status(), StatusCode::NO_CONTENT);
        assert_eq!(downstream.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn scanner_counter_has_fixed_cardinality() {
        let before = rejected_total();
        assert!(before <= u64::MAX);
        // No label includes IP, path, email, cookie, user-agent or session.
        assert_eq!(
            std::mem::size_of_val(&REJECTED),
            std::mem::size_of::<AtomicU64>()
        );
    }
}
