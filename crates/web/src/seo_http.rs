//! SEO-06: first read-only HTTP projection. No public indexing is activated here.
//! The routes remain useful for pre-release tests while Book HTML is noindex.
use super::AppState;
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Response},
};
use tor_search::domain::seo_urls::TrustedSeoOrigin;

fn trusted_origin() -> Option<TrustedSeoOrigin> {
    let value = std::env::var("TOR_PUBLIC_ORIGIN").ok()?;
    TrustedSeoOrigin::from_config(&value)
}

/// Do not advertise indexable URLs until SEO-04/05 HTML activation is accepted.
/// This gate is deliberately not implemented by this PR.
fn indexing_active() -> bool {
    false
}

pub(super) async fn robots_txt() -> Response {
    let mut body = String::from("User-agent: *\nAllow: /\n");
    if indexing_active() {
        if let Some(origin) = trusted_origin() {
            // The origin is parsed from trusted configuration, never Host.
            if let Some(book) = tor_search::domain::seo_urls::BookUrl::from_id(1) {
                let url = origin.book_url(book);
                let base = url.trim_end_matches("/books/1");
                body.push_str(&format!("Sitemap: {base}/sitemap.xml\n"));
            }
        }
    }
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8"),
         (header::CACHE_CONTROL, "public, max-age=300")],
        body,
    )
        .into_response()
}

pub(super) async fn sitemap_xml(State(_state): State<AppState>) -> Response {
    // Empty and non-advertised until real per-Book F facts and approved HTML
    // agree on indexability. An empty map is safer than publishing private data.
    let xml = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\"></urlset>\n";
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8"),
         (header::CACHE_CONTROL, "no-store")],
        xml,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn seo_06_has_no_early_indexing() {
        assert!(!indexing_active());
        let r = robots_txt().await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(r.headers()[header::CONTENT_TYPE], "text/plain; charset=utf-8");
    }

    #[test]
    fn seo_06_only_accepts_trusted_https_origin() {
        assert!(TrustedSeoOrigin::from_config("https://pandaolate.ru").is_some());
        assert!(TrustedSeoOrigin::from_config("https://pandaolate.ru@evil.example").is_none());
    }
}
