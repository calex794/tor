use super::{catalog_taxonomy_selects, escape_html, CatalogPageQuery};

pub(super) fn render(request: &CatalogPageQuery<'_>) -> String {
    let extra_count = [request.genre, request.tag, request.fandom, request.language]
        .into_iter()
        .flatten()
        .filter(|value| !value.trim().is_empty())
        .count();
    let sort_options = options(
        &[
            ("oldest", "Сначала добавленные"),
            ("newest", "Сначала новые"),
            ("title", "По названию"),
            ("author", "По автору"),
        ],
        request.sort,
        false,
    );
    let language_options = options(
        &[
            ("", "Все языки"),
            ("ru", "Русский"),
            ("en", "Английский"),
            ("zh", "Китайский"),
            ("ko", "Корейский"),
            ("ja", "Японский"),
            ("vi", "Вьетнамский"),
        ],
        request.language.unwrap_or(""),
        true,
    );
    let reset = if extra_count > 0 || !request.query.trim().is_empty() {
        "<a class=\"catalog-filters__reset\" href=\"/catalog\">Сбросить</a>"
    } else {
        ""
    };
    let count = if extra_count == 0 {
        String::new()
    } else {
        format!(" <span class=\"catalog-filters__count\">{extra_count}</span>")
    };
    let form = include_str!("../../../templates/catalog_filters.html")
        .replace("<!--QUERY-->", &escape_html(request.query))
        .replace("<!--GENRE-->", &escape_html(request.genre.unwrap_or("")))
        .replace("<!--TAG-->", &escape_html(request.tag.unwrap_or("")))
        .replace("<!--FANDOM-->", &escape_html(request.fandom.unwrap_or("")))
        .replace("<!--SORT_OPTIONS-->", &sort_options)
        .replace("<!--LANGUAGE_OPTIONS-->", &language_options)
        .replace(
            "<!--ADVANCED_OPEN-->",
            if extra_count > 0 { " open" } else { "" },
        )
        .replace("<!--FILTER_COUNT-->", &count)
        .replace("<!--RESET_LINK-->", reset);
    catalog_taxonomy_selects::apply(
        form,
        request.options,
        request.genre,
        request.tag,
        request.fandom,
    )
}

fn options(values: &[(&str, &str)], current: &str, keep_unknown: bool) -> String {
    let mut out = String::new();
    for (value, label) in values {
        let selected = if *value == current { " selected" } else { "" };
        out.push_str(&format!(
            "<option value=\"{}\"{selected}>{}</option>",
            escape_html(value),
            escape_html(label),
        ));
    }
    if keep_unknown && !values.iter().any(|(value, _)| *value == current) {
        let safe = escape_html(current);
        out.push_str(&format!(
            "<option value=\"{safe}\" selected>{safe}</option>"
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query() -> CatalogPageQuery<'static> {
        CatalogPageQuery {
            query: "",
            language: None,
            genre: None,
            tag: None,
            fandom: None,
            cursor: None,
            previous_cursor: None,
            page_number: 1,
            sort: "oldest",
            total: Some(0),
            options: None,
        }
    }

    #[test]
    fn initial_catalog_keeps_search_available_and_extra_filters_collapsed() {
        let html = render(&query());
        assert!(html.contains("class=\"catalog-filters__advanced\">"));
        assert!(!html.contains("catalog-filters__reset"));
        assert!(html.contains("name=\"q\""));
        assert!(html.contains("<option value=\"ru\">Русский</option>"));
    }

    #[test]
    fn selected_filters_are_expanded_and_preserved_without_scripts() {
        let mut request = query();
        request.genre = Some("Фэнтези");
        request.language = Some("ru");
        request.sort = "newest";
        let html = render(&request);
        assert!(html.contains("class=\"catalog-filters__advanced\" open>"));
        assert!(html.contains("class=\"catalog-filters__count\">2</span>"));
        assert!(html.contains("value=\"Фэнтези\" selected"));
        assert!(html.contains("value=\"ru\" selected>Русский"));
        assert!(html.contains("value=\"newest\" selected"));
    }

    #[test]
    fn unknown_language_and_query_are_preserved_as_escaped_text() {
        let mut request = query();
        request.language = Some("\"><script>bad</script>");
        request.query = "\" onfocus=\"bad";
        let html = render(&request);
        assert!(!html.contains("<script>"));
        assert!(!html.contains(" onfocus=\""));
        assert!(html.contains("&lt;script&gt;bad&lt;/script&gt;"));
    }
}
