//! B-18: accessible previous/next navigation with only bounded cursor keys.
//! Unlike OFFSET pagination, neither direction scans skipped pages.
use super::{encode_search_term, CatalogPageQuery};
use std::fmt::Write;

pub(super) fn render(request: &CatalogPageQuery<'_>) -> String {
    let mut params = Vec::new();
    for (name, value) in [
        ("q", request.query),
        ("language", request.language.unwrap_or("")),
        ("genre", request.genre.unwrap_or("")),
        ("tag", request.tag.unwrap_or("")),
        ("fandom", request.fandom.unwrap_or("")),
    ] {
        if !value.is_empty() {
            params.push(format!("{name}={}", encode_search_term(value)));
        }
    }
    // The route validates the four supported sort values before rendering.
    params.push(format!("sort={}", encode_search_term(request.sort)));
    let base = format!("/catalog?{}", params.join("&amp;"));
    let mut out = String::from(
        r#"<nav class="catalog-pagination" aria-label="Навигация по страницам каталога">"#,
    );
    if request.page_number > 1 {
        write!(
            out,
            r#"<a class="ui-button ui-button--ghost" href="{base}">В начало</a>"#,
        )
        .expect("format pagination");
    }
    if let Some(before) = request.previous_cursor {
        let prev = request.page_number.saturating_sub(1).max(1);
        write!(
            out,
            r#"<a class="ui-button ui-button--ghost" rel="prev" href="{base}&amp;before={}&amp;page={prev}">Назад</a>"#,
            encode_search_term(before),
        )
        .expect("format previous page");
    }
    write!(
        out,
        r#"<span class="catalog-pagination__current" aria-current="page">Страница {}</span>"#,
        request.page_number,
    )
    .expect("format current page");
    if let Some(after) = request.cursor {
        if let Some(next) = request.page_number.checked_add(1) {
            write!(
                out,
                r#"<a class="ui-button ui-button--ghost" rel="next" href="{base}&amp;after={}&amp;page={next}">Вперёд</a>"#,
                encode_search_term(after),
            )
            .expect("format next page");
        }
    }
    out.push_str("</nav>");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query<'a>(
        after: Option<&'a str>,
        before: Option<&'a str>,
        number: u32,
    ) -> CatalogPageQuery<'a> {
        CatalogPageQuery {
            query: "кот & мышь",
            language: Some("ru"),
            genre: Some("Фэнтези"),
            tag: Some("a&b"),
            fandom: Some("любимый"),
            cursor: after,
            previous_cursor: before,
            page_number: number,
            sort: "title",
            total: None,
            options: None,
        }
    }

    #[test]
    fn b_18_first_page_has_only_forward_navigation() {
        let html = render(&query(Some("title.6162.20"), None, 1));
        assert!(html.contains("Страница 1"));
        assert!(!html.contains("rel=\"prev\""));
        assert!(!html.contains(">В начало</a>"));
        assert!(html.contains("rel=\"next\""));
        assert!(html.contains("after=title.6162.20&amp;page=2"));
        for parameter in [
            "q=%D0%BA%D0%BE%D1%82%20%26%20%D0%BC%D1%8B%D1%88%D1%8C",
            "language=ru",
            "genre=%D0%A4%D1%8D%D0%BD%D1%82%D0%B5%D0%B7%D0%B8",
            "tag=a%26b",
            "sort=title",
        ] {
            assert!(html.contains(parameter), "{parameter}");
        }
    }

    #[test]
    fn b_18_middle_and_last_pages_preserve_filters_and_direction() {
        let middle = render(&query(Some("title.ffff.40"), Some("title.6162.21"), 2));
        assert!(middle.contains("rel=\"prev\""));
        assert!(middle.contains("before=title.6162.21&amp;page=1"));
        assert!(middle.contains("after=title.ffff.40&amp;page=3"));
        assert!(middle.contains("Страница 2"));
        assert!(middle.contains(">В начало</a>"));
        let last = render(&query(None, Some("title.aaaa.41"), 3));
        assert!(last.contains("before=title.aaaa.41&amp;page=2"));
        assert!(!last.contains("rel=\"next\""));
        assert!(last.contains("Страница 3"));
    }

    #[test]
    fn b_18_cursor_is_url_encoded_never_html_injected() {
        let html = render(&query(Some("<x&\""), Some("\"><script>"), 2));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("before=\"><script>"));
        assert!(html.contains("before=%22%3E%3Cscript%3E"));
        assert!(html.contains("after=%3Cx%26%22"));
    }
}
