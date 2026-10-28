//! H-10: restore NEwBOO's native taxonomy dropdowns from public-only data.
use super::escape_html;
use tor_search::repositories::catalog::TaxonomyOptions;

/// Replace only the three recognized, already escaped catalogue inputs.
pub(super) fn apply(
    mut html: String,
    options: Option<&TaxonomyOptions>,
    genre: Option<&str>,
    tag: Option<&str>,
    fandom: Option<&str>,
) -> String {
    let kinds = [
        (
            "book-genre",
            "genre",
            "Жанр",
            "Все жанры",
            genre,
            options.map(|o| &o.genres),
        ),
        (
            "book-tag",
            "tag",
            "Тег",
            "Все теги",
            tag,
            options.map(|o| &o.tags),
        ),
        (
            "book-fandom",
            "fandom",
            "Фэндом",
            "Все фэндомы",
            fandom,
            options.map(|o| &o.fandoms),
        ),
    ];
    for (id, key, label, empty, current, available) in kinds {
        let Some(at) = html.find(&format!("id=\"{id}\"")) else {
            continue;
        };
        let Some(start) = html[..at].rfind("<input ") else {
            continue;
        };
        let Some(end) = html[at..].find('>').map(|offset| at + offset + 1) else {
            continue;
        };
        let mut select = format!(
            "<select class=\"ui-select catalog-filters__select\" id=\"{id}\" name=\"{key}\" aria-label=\"{label}\"><option value=\"\">{empty}</option>"
        );
        let selected = current.filter(|v| !v.is_empty());
        let mut seen_selected = false;
        if let Some(values) = available {
            for value in values {
                if value.is_empty() {
                    continue;
                }
                let escaped = escape_html(value);
                let chosen = selected == Some(value.as_str());
                seen_selected |= chosen;
                let attr = if chosen { " selected" } else { "" };
                select.push_str(&format!(
                    "<option value=\"{escaped}\"{attr}>{escaped}</option>"
                ));
            }
        }
        // Cursor pages need not refetch taxonomy. Preserve the selected value
        // even when it is outside the bounded first-page options.
        if let Some(value) = selected.filter(|_| !seen_selected) {
            let escaped = escape_html(value);
            select.push_str(&format!(
                "<option value=\"{escaped}\" selected>{escaped}</option>"
            ));
        }
        select.push_str("</select>");
        html.replace_range(start..end, &select);
    }
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form() -> String {
        "<form><input class=\"ui-input\" id=\"book-genre\" type=\"text\" name=\"genre\" value=\"\"><input class=\"ui-input\" id=\"book-tag\" type=\"text\" name=\"tag\" value=\"\"><input class=\"ui-input\" id=\"book-fandom\" type=\"text\" name=\"fandom\" value=\"\"></form>".to_owned()
    }

    #[test]
    fn h_10_catalog_dropdowns_preserve_selected_options() {
        let options = TaxonomyOptions {
            genres: vec!["Фэнтези".into(), "Фантастика".into()],
            tags: vec!["Попаданцы".into()],
            fandoms: vec!["Аниме".into()],
        };
        let result = apply(
            form(),
            Some(&options),
            Some("Фэнтези"),
            Some("Попаданцы"),
            None,
        );
        assert!(result.contains("<option value=\"Фэнтези\" selected>Фэнтези</option>"));
        assert!(result.contains("<option value=\"Попаданцы\" selected>Попаданцы</option>"));
        assert!(result.contains("Все фэндомы"));
        assert!(!result.contains("<datalist"));
    }

    #[test]
    fn h_10_cursor_page_preserves_current_selection_without_refetch() {
        let result = apply(form(), None, Some("Фэнтези"), None, None);
        assert!(result.contains("<option value=\"Фэнтези\" selected>Фэнтези</option>"));
    }

    #[test]
    fn h_11_taxonomy_escapes_untrusted_pg_and_query_values() {
        let options = TaxonomyOptions {
            genres: vec!["\" onclick=\"x".into()],
            tags: vec!["<script>x</script>".into()],
            fandoms: vec![],
        };
        let result = apply(form(), Some(&options), Some("\" onclick=\"x"), None, None);
        assert!(!result.contains("<script>"));
        assert!(!result.contains("onclick=\"x"));
        assert!(result.contains("&quot; onclick=&quot;x"));
        assert!(result.contains("&lt;script&gt;"));
    }
}
