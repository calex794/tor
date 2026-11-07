//! S025 real CDN HTML projection against disposable PostgreSQL and MinIO.
use super::{request, BOOK_ID};
use axum::{body::to_bytes, http::StatusCode};
use std::sync::Arc;
use tor_books::repositories::{cover_slots::PublicCoverKind, PgCatalog};
use tor_ingest::repositories::s3::S3CoverStore;
use tor_reading::repositories::PgReader;

pub(super) async fn verify(
    catalog: &PgCatalog,
    publisher: &S3CoverStore,
    public_urls: &[(PublicCoverKind, String)],
) {
    // S025: real PG ledger + private MinIO + a separate mock public bucket,
    // with the exact public HTML routes used by visitors. This test is run
    // --ignored --test-threads=1 so changing the process-local flag is isolated.
    // It never contacts the configured HTTPS CDN host.
    {
        struct RestoreFlag(Option<std::ffi::OsString>);
        impl Drop for RestoreFlag {
            fn drop(&mut self) {
                if let Some(previous) = self.0.take() {
                    std::env::set_var("TOR_CDN_COVER_HTML_ENABLED", previous);
                } else {
                    std::env::remove_var("TOR_CDN_COVER_HTML_ENABLED");
                }
            }
        }
        let _restore = RestoreFlag(std::env::var_os("TOR_CDN_COVER_HTML_ENABLED"));
        std::env::remove_var("TOR_CDN_COVER_HTML_ENABLED");
        let legacy_app = tor_web::router_with_postgres_reading_covers(
            catalog.clone(),
            PgReader::new(catalog.pool()),
            Arc::new(publisher.clone()),
        );
        let routes = [
            ("/", PublicCoverKind::Card, "card"),
            ("/catalog", PublicCoverKind::Card, "card"),
            ("/books/950", PublicCoverKind::Main, "main"),
        ];
        for (path, _, variant) in routes {
            let response = request(&legacy_app, path).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}: legacy HTML");
            let body = to_bytes(response.into_body(), 262_144).await.unwrap();
            let html = String::from_utf8_lossy(&body);
            let expected = format!("src=\"/covers/{BOOK_ID}/{variant}.webp\"");
            assert!(
                html.contains(&expected),
                "{path}: default-off CDN must preserve the approved origin cover"
            );
            assert!(!html.contains("https://cdn.example/public/covers/"));
        }

        std::env::set_var("TOR_CDN_COVER_HTML_ENABLED", "1");
        let enabled_app = tor_web::router_with_postgres_reading_covers(
            catalog.clone(),
            PgReader::new(catalog.pool()),
            Arc::new(publisher.clone()),
        );
        for (path, kind, variant) in routes {
            let expected_url = public_urls
                .iter()
                .find(|(published_kind, _)| *published_kind == kind)
                .map(|(_, url)| url)
                .unwrap();
            let response = request(&enabled_app, path).await;
            assert_eq!(response.status(), StatusCode::OK, "{path}: CDN HTML");
            if let Some(csp) = response.headers().get("content-security-policy") {
                let csp = csp.to_str().unwrap();
                assert!(
                    csp.contains("img-src 'self' https://cdn.example;"),
                    "{path}: allow only the verified image origin"
                );
                assert!(csp.contains("script-src 'none'"));
            }
            let body = to_bytes(response.into_body(), 262_144).await.unwrap();
            let html = String::from_utf8_lossy(&body);
            assert!(
                html.contains(&format!("src=\"{expected_url}\"")),
                "{path}: current B-approved copied cover must use its immutable CDN URL"
            );
            assert!(
                !html.contains(&format!("src=\"/covers/{BOOK_ID}/{variant}.webp\"")),
                "{path}: successful CDN projection must replace the old origin src"
            );
            assert!(!html.contains("staging/covers/"));
        }
    }
}
