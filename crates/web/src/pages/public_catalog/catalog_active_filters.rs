//! Presentational NEwBOO active filter chips; no additional catalog queries.
use super::{encode_search_term, escape_html, CatalogPageQuery};
use std::fmt::Write;

pub(super) fn render(request: &CatalogPageQuery<'_>) -> String {
    let fields = [
        ("Поиск", "q", request.query),
        ("Язык", "language", request.language.unwrap_or("")),
        ("Жанр", "genre", request.genre.unwrap_or("")),
        ("Тег", "tag", request.tag.unwrap_or("")),
        ("Фэндом", "fandom", request.fandom.unwrap_or("")),
    ];
    let active: Vec<_> = fields
        .into_iter()
        .filter(|(_, _, value)| !value.trim().is_empty())
        .collect();
    if active.is_empty() {
        return String::new();
    }

    let mut html = String::from(
        "<nav class=\"catalog-active-filters\" aria-label=\"Активные фильтры\">\
         <span class=\"catalog-active-filters__label\">Активные фильтры</span>\
         <div class=\"catalog-active-filters__list\">",
    );
    for (removed, (label, _, value)) in active.iter().enumerate() {
        // Removing a filter restarts at page one, but keeps every other
        // selected filter and the supported sort order.
        let mut params: Vec<String> = active
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != removed)
            .map(|(_, (_, key, value))| format!("{key}={}", encode_search_term(value)))
            .collect();
        if matches!(request.sort, "oldest" | "newest" | "title" | "author")
            && request.sort != "oldest"
        {
            params.push(format!("sort={}", request.sort));
        }
        let href = if params.is_empty() {
            "/catalog".to_owned()
        } else {
            format!("/catalog?{}", params.join("&amp;"))
        };
        let safe_label = escape_html(label);
        let safe_value = escape_html(value);
        write!(
            html,
            r#"<a class="ui-chip ui-chip--active" href="{href}" aria-label="Убрать фильтр {safe_label}: {safe_value}"><span class="ui-chip__key">{safe_label}</span><span class="ui-chip__value">{safe_value}</span><span class="ui-chip__remove" aria-hidden="true">×</span></a>"#
        )
        .expect("String write");
    }
    html.push_str("</div></nav>");
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query<'a>(
        term: &'a str,
        language: Option<&'a str>,
        genre: Option<&'a str>,
        sort: &'a str,
    ) -> CatalogPageQuery<'a> {
        CatalogPageQuery {
            query: term,
            language,
            genre,
            tag: None,
            fandom: None,
            cursor: Some("private-cursor-should-not-survive"),
            sort,
            total: Some(7),
            options: None,
        }
    }

    #[test]
    fn h_10_empty_catalog_has_no_active_filter_navigation() {
        assert!(render(&query("", None, None, "oldest")).is_empty());
        assert!(render(&query("  ", Some(""), None, "newest")).is_empty());
    }

    #[test]
    fn h_10_remove_one_filter_preserves_other_filters_and_sort_without_cursor() {
        let markup = render(&query(
            "кот & мышь",
            Some("ru"),
            Some("Научная фантастика"),
            "newest",
        ));
        assert_eq!(markup.matches("ui-chip--active").count(), 3);
        assert!(markup.contains("Активные фильтры"));
        assert!(markup.contains("Убрать фильтр Поиск: кот &amp; мышь"));
        assert!(markup.contains(
            "href=\"/catalog?language=ru&amp;genre=%D0%9D%D0%B0%D1%83%D1%87%D0%BD%D0%B0%D1%8F%20%D1%84%D0%B0%D0%BD%D1%82%D0%B0%D1%81%D1%82%D0%B8%D0%BA%D0%B0&amp;sort=newest\""
        ));
        assert!(markup.contains("q=%D0%BA%D0%BE%D1%82%20%26%20%D0%BC%D1%8B%D1%88%D1%8C"));
        assert!(!markup.contains("after="));
        assert!(!markup.contains("private-cursor"));
    }

    #[test]
    fn h_10_last_filter_removal_links_to_unfiltered_catalog() {
        let markup = render(&query("новелла", None, None, "oldest"));
        assert!(markup.contains("href=\"/catalog\""));
        assert_eq!(markup.matches("ui-chip--active").count(), 1);
    }

    #[test]
    fn h_11_filter_values_cannot_inject_html_or_link_attributes() {
        let markup = render(&query(
            "<script>alert(1)</script>",
            None,
            Some("\" onmouseover=\"evil"),
            "newest",
        ));
        assert!(!markup.contains("<script>"));
        assert!(!markup.contains("onmouseover=\"evil"));
        assert!(markup.contains("&lt;script&gt;"));
        assert!(markup.contains("&quot; onmouseover=&quot;evil"));
        assert!(markup.contains("%3Cscript%3Ealert%281%29%3C%2Fscript%3E"));
    }
}
