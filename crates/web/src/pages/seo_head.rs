//! SEO-04a: render conservative server-side metadata for public book pages.
//! F decides indexability. H only escapes approved content and emits HTML.
//! Do not activate SEO mode until real B/C and Shadow ownership facts are wired.
use super::escape_html;
use std::fmt::Write;
use tor_search::{
    domain::{
        seo::{
            SeoBookFacts, SeoBookView, SeoDecision, SeoMode, SeoPage, SeoSite, SeoVisibility,
        },
        seo_urls::{BookUrl, TrustedSeoOrigin},
    },
    services::seo::SeoPolicy,
};

pub(super) fn default_head() -> &'static str {
    // No SEO policy is connected to arbitrary/private pages: fail closed.
    // Preserve the H-11 error-page contract with exactly one robots tag.
    "<meta name=\"robots\" content=\"noindex, nofollow\">"
}

/// A book's visible title and description come only from the already authorized
/// public B/C page. No caller-supplied Host/Forwarded header is accepted here.
pub(super) fn disabled_book_head(
    book_id: u64,
    has_public_free_chapter: bool,
    title: &str,
    description: &str,
) -> String {
    let decision = SeoPolicy::new(SeoSite::Pandaolate, SeoMode::Disabled).decide(
        SeoPage::Book {
            book: SeoBookFacts {
                book_id,
                visibility: SeoVisibility::AnonymousPublic,
                has_public_content: has_public_free_chapter,
                // Placeholder cannot accidentally enable indexing: mode is
                // hard-disabled until verified owner and pilot facts are added.
                index_owner: SeoSite::Pandaolate,
                shadow_enabled: false,
                pilot_approved: false,
            },
            view: SeoBookView::Canonical,
        },
    );
    render_book_head(title, description, &decision, None)
}

fn compact(value: &str, max_chars: usize) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(max_chars)
        .collect()
}

/// A reusable renderer for eventual, explicitly reviewed Public/Pilot modes.
/// Canonical and og:url require BOTH an F-approved canonical path and a trusted
/// HTTPS origin. Absent either, metadata remains noindex-only with no URL.
fn render_book_head(
    title: &str,
    description: &str,
    decision: &SeoDecision,
    origin: Option<&TrustedSeoOrigin>,
) -> String {
    let title = escape_html(&compact(title, 180));
    let description = escape_html(&compact(description, 200));
    // A positive SEO decision is not enough to index a page: a trusted
    // absolute canonical URL is mandatory. Never infer it from Host headers.
    let trusted_url = decision
        .canonical_path
        .as_deref()
        .filter(|_| decision.indexable())
        .and_then(BookUrl::from_exact_path)
        .zip(origin)
        .map(|(book, origin)| escape_html(&origin.book_url(book)));
    let robots = if decision.indexable() && trusted_url.is_none() {
        "noindex,nofollow"
    } else {
        decision.robots.meta()
    };
    let mut html = format!("<meta name=\"robots\" content=\"{robots}\">");
    write!(
        html,
        "\n<meta property=\"og:type\" content=\"book\">\
         \n<meta property=\"og:title\" content=\"{title}\">\
         \n<meta property=\"og:description\" content=\"{description}\">\
         \n<meta name=\"twitter:card\" content=\"summary\">\
         \n<meta name=\"twitter:title\" content=\"{title}\">\
         \n<meta name=\"twitter:description\" content=\"{description}\">"
    )
    .expect("write metadata");
    if let Some(url) = trusted_url {
        write!(
            html,
            "\n<link rel=\"canonical\" href=\"{url}\">\
             \n<meta property=\"og:url\" content=\"{url}\">"
        )
        .expect("write safe canonical");
    }
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    fn public_decision(id: u64) -> SeoDecision {
        SeoPolicy::new(SeoSite::Pandaolate, SeoMode::Public).decide(SeoPage::Book {
            book: SeoBookFacts {
                book_id: id,
                visibility: SeoVisibility::AnonymousPublic,
                has_public_content: true,
                index_owner: SeoSite::Pandaolate,
                shadow_enabled: false,
                pilot_approved: false,
            },
            view: SeoBookView::Canonical,
        })
    }

    #[test]
    fn seo_04_authorized_canonical_uses_only_trusted_https_origin() {
        let origin = TrustedSeoOrigin::from_config("https://pandaolate.ru").unwrap();
        let html = render_book_head("Название", "Описание", &public_decision(42), Some(&origin));
        assert_eq!(html.matches("rel=\"canonical\"").count(), 1);
        assert_eq!(html.matches("property=\"og:url\"").count(), 1);
        assert!(html.contains("https://pandaolate.ru/books/42"));
        assert!(!html.contains("http://"));
        assert!(TrustedSeoOrigin::from_config("https://pandaolate.ru@evil.example").is_none());
    }

    #[test]
    fn seo_04_untrusted_title_and_description_are_escaped() {
        let text = "\" onmouseover=\"evil\"><script>alert(1)</script>";
        let html = render_book_head(text, text, &public_decision(42), None);
        assert!(html.contains("&quot;"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("onmouseover=\"evil"));
        assert!(!html.contains("rel=\"canonical\""));
    }

    #[test]
    fn seo_04_live_book_pages_keep_indexing_disabled() {
        let html = disabled_book_head(7, true, "Книга", "Открытый текст");
        assert!(html.contains("noindex,follow"));
        assert!(!html.contains("rel=\"canonical\""));
        assert!(!html.contains("property=\"og:url\""));
        let no_free = disabled_book_head(7, false, "Книга", "");
        assert!(no_free.contains("noindex,nofollow"));
    }

    #[test]
    fn seo_04_positive_policy_without_trusted_origin_stays_noindex() {
        let html = render_book_head("Название", "Описание", &public_decision(42), None);
        assert!(html.contains("name=\"robots\" content=\"noindex,nofollow\""));
        assert!(!html.contains("rel=\"canonical\""));
        assert!(!html.contains("property=\"og:url\""));
    }

    #[test]
    fn seo_04_full_book_html_has_safe_metadata_and_one_robots_tag() {
        let html = super::super::book_detail(&super::super::PublicBookCard {
            id: 42,
            title: "<script>private?</script>".to_owned(),
            language: "ru".to_owned(),
            author_name: None,
            genres: Vec::new(),
        })
        .0;
        assert_eq!(html.matches("name=\"robots\"").count(), 1);
        assert!(html.contains("name=\"robots\" content=\"noindex,nofollow\""));
        assert!(html.contains("property=\"og:title\""));
        assert!(html.contains("&lt;script&gt;private?&lt;/script&gt;"));
        assert!(!html.contains("<script>private?</script>"));
        assert!(!html.contains("rel=\"canonical\""));
    }

    #[test]
    fn seo_04_shared_layout_inserts_exactly_one_robots_tag() {
        let html = super::super::layout("Личный кабинет", "Описание", "<h1>Тест</h1>").0;
        assert_eq!(html.matches("name=\"robots\"").count(), 1);
        assert!(html.contains("noindex, nofollow"));
        assert!(!html.contains("rel=\"canonical\""));
        assert!(!html.contains("PAGE_SEO_HEAD"));
    }
}
