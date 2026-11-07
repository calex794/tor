//! SEO-06: read-only public sitemap and robots projection.
//! Indexing stays disabled unless the operator explicitly enables the
//! public mode AND confirms the HTTP publication barrier.
use super::{AppState, pages};
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
};
use tor_search::{
    domain::seo::{
        SeoBookFacts, SeoBookView, SeoMode, SeoPage, SeoSite, SeoVisibility,
    },
    domain::seo_urls::{BookUrl, TrustedSeoOrigin},
    services::seo::SeoPolicy,
};

fn trusted_origin() -> Option<TrustedSeoOrigin> {
    let value = std::env::var("TOR_PUBLIC_ORIGIN").ok()?;
    TrustedSeoOrigin::from_config(&value)
}

fn indexing_active() -> bool {
    std::env::var("TOR_SEO_MODE").as_deref() == Ok("public")
        && std::env::var("TOR_SEO_HTTP_READY").as_deref() == Ok("1")
        && trusted_origin().is_some()
}

fn base_url(origin: &TrustedSeoOrigin) -> String {
    let book = BookUrl::from_id(1).expect("constant positive book id");
    origin
        .book_url(book)
        .trim_end_matches("/books/1")
        .to_owned()
}

fn xml_for_books(base: &str, ids: &[u64]) -> String {
    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<urlset xmlns=\"http://www.sitemaps.org/schemas/sitemap/0.9\">\n",
    );
    for &id in ids {
        if let Some(book) = BookUrl::from_id(id) {
            xml.push_str(&format!("  <url><loc>{base}{}</loc></url>\n", book.path()));
        }
    }
    xml.push_str("</urlset>\n");
    xml
}

pub(super) async fn robots_txt() -> Response {
    let mut body = String::from("User-agent: *\nAllow: /\n");
    if indexing_active() {
        if let Some(origin) = trusted_origin() {
            body.push_str(&format!("Sitemap: {}/sitemap.xml\n", base_url(&origin)));
        }
    }
    (
        [
            (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
        ],
        body,
    )
        .into_response()
}

pub(super) async fn sitemap_xml(State(state): State<AppState>) -> Response {
    let mut ids = Vec::new();
    let mut origin = None;
    if indexing_active() {
        origin = trusted_origin();
        let Some(search) = state.search.as_ref() else {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        };
        match search.seo_public_book_ids().await {
            Ok(found) => ids = found,
            Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
        }
    }
    let base = origin.as_ref().map(base_url).unwrap_or_default();
    let xml = xml_for_books(&base, &ids);
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/xml; charset=utf-8"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        xml,
    )
        .into_response()
}

/// Only books accepted by the same F/PG gate as the sitemap may advertise
/// a canonical URL. It never changes read access or private HTML.
pub(super) async fn decorate_book(
    state: &AppState,
    book_id: u64,
    html: Html<String>,
) -> Html<String> {
    if !indexing_active() {
        return html;
    }
    let Some(search) = state.search.as_ref() else {
        return html;
    };
    if !matches!(search.seo_public_book_indexable(book_id).await, Ok(true)) {
        return html;
    }
    let Some(origin) = trusted_origin() else {
        return html;
    };
    let decision = SeoPolicy::new(SeoSite::Pandaolate, SeoMode::Public).decide(SeoPage::Book {
        book: SeoBookFacts {
            book_id,
            visibility: SeoVisibility::AnonymousPublic,
            has_public_content: true,
            index_owner: SeoSite::Pandaolate,
            shadow_enabled: false,
            pilot_approved: false,
        },
        view: SeoBookView::Canonical,
    });
    let Some(path) = decision.canonical_path.as_deref().filter(|_| decision.indexable()) else {
        return html;
    };
    let Some(book_url) = BookUrl::from_exact_path(path) else {
        return html;
    };
    let url = pages::escape_html(&origin.book_url(book_url));
    let old = "<meta name=\"robots\" content=\"noindex,follow\">";
    let old_strict = "<meta name=\"robots\" content=\"noindex,nofollow\">";
    let replacement = format!(
        "<meta name=\"robots\" content=\"index,follow\">\n<link rel=\"canonical\" href=\"{url}\">\n<meta property=\"og:url\" content=\"{url}\">",
    );
    if html.0.contains(old) {
        Html(html.0.replacen(old, &replacement, 1))
    } else if html.0.contains(old_strict) {
        Html(html.0.replacen(old_strict, &replacement, 1))
    } else {
        // Do not generate a second robots tag or an unaudited canonical.
        html
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seo_06_xml_is_bounded_and_uses_only_numeric_book_identity() {
        let xml = xml_for_books("https://pandaolate.ru", &[1, 42, 0]);
        assert_eq!(xml.matches("<loc>").count(), 2);
        assert!(xml.contains("<loc>https://pandaolate.ru/books/42</loc>"));
        assert!(!xml.contains("/books/0</loc>"));
    }

    #[test]
    fn seo_06_trusted_origin_rejects_host_poisoning() {
        assert!(TrustedSeoOrigin::from_config("https://pandaolate.ru").is_some());
        assert!(TrustedSeoOrigin::from_config("https://pandaolate.ru@evil.example").is_none());
    }
}
