//! Shared organic/editorial chapter-end BookCard markup. Source attribution
//! remains on the enclosing article, never on the reader-facing card.
use crate::pages::escape_html;
use std::fmt::Write;
use tor_recommendations::repositories::feedback::ShelfCardPreview;

pub(crate) fn markup(id: i64, title: &str, preview: Option<&ShelfCardPreview>) -> String {
    let title = escape_html(title);
    let mut details = String::new();
    if let Some(preview) = preview.filter(|value| value.book_id == id) {
        if !preview.genres.is_empty() {
            let genres = preview
                .genres
                .iter()
                .map(|genre| escape_html(genre))
                .collect::<Vec<_>>()
                .join(" · ");
            let _ = write!(
                details,
                "<p class=\"book-mini-card__flip-genres\">{genres}</p>"
            );
        }
        if !preview.description.trim().is_empty() {
            let _ = write!(
                details,
                "<p class=\"book-mini-card__flip-description\">{}</p>",
                escape_html(preview.description.trim())
            );
        }
        if preview.work_rating.is_some() || preview.published_chapters > 0 {
            details.push_str("<div class=\"book-mini-card__flip-stats\">");
            if let Some(rating) = &preview.work_rating {
                let _ = write!(
                    details,
                    "<span aria-label=\"Рейтинг книги\">★ {}</span>",
                    escape_html(rating)
                );
            }
            if preview.published_chapters > 0 {
                let _ = write!(details, "<span>Глав: {}</span>", preview.published_chapters);
            }
            details.push_str("</div>");
        }
    }
    format!(
        r#"<div class="book-mini-card__visual" data-reader-shelf-card>
<div class="book-mini-card__flip-inner">
<div class="book-mini-card__flip-front">
<a class="book-mini-card__cover-link" data-flow-link href="/books/{id}" aria-label="{title}"><div class="book-mini-card__cover book-shimmer"><img src="/covers/{id}/card.webp" alt="" loading="lazy" decoding="async" draggable="false"></div></a>
</div>
<div class="book-mini-card__flip-back"><strong class="book-mini-card__flip-title">{title}</strong>{details}<a class="book-mini-card__flip-open" data-flow-link href="/books/{id}">Открыть книгу</a></div>
</div>
<button class="book-mini-card__flip-info" type="button" data-reader-shelf-info aria-expanded="false" aria-label="Информация о книге: {title}">i</button>
</div><div class="book-mini-card__body"><h3 class="book-mini-card__title"><a href="/books/{id}" data-flow-link>{title}</a></h3></div>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flip_has_real_data_safe_links_and_no_public_source_labels() {
        let metadata = ShelfCardPreview {
            book_id: 12,
            description: "<script>alert(1)</script>".into(),
            genres: vec!["Система & магия".into()],
            work_rating: Some("4.5".into()),
            published_chapters: 126,
        };
        let html = markup(12, "<Название>", Some(&metadata));
        assert!(html.contains("data-reader-shelf-info"));
        assert!(html.contains("aria-expanded=\"false\""));
        assert!(html.contains("Система &amp; магия"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("★ 4.5"));
        assert!(html.contains("Глав: 126"));
        assert!(html.contains("/covers/12/card.webp"));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("Выбор редакции"));
        assert!(!html.contains("Другие авторы"));
    }

    #[test]
    fn missing_metadata_does_not_invent_rating_or_chapters() {
        let html = markup(12, "Название", None);
        assert!(html.contains("Открыть книгу"));
        assert!(!html.contains("Глав:"));
        assert!(!html.contains("★"));
        assert!(!html.contains("book-mini-card__flip-description"));
    }
}
