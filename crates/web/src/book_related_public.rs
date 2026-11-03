//! S020: render only explicit live-public related-book links on a public page.
//! The B-owned repository resolves authoritative visibility; no client-derived ids.
use crate::pages;
use axum::{http::StatusCode, response::Html};
use tor_books::repositories::{postgres::RelatedBookCard, PgCatalog};

fn render(related: &[RelatedBookCard]) -> String {
    let mut entries = String::new();
    for book in related.iter().take(12).filter(|book| book.id > 0) {
        entries.push_str(&format!(
            "<li class=\"book-related-grid__item\">             <a class=\"book-related-grid__link\" href=\"/books/{}\">{}</a>             </li>",
            book.id,
            pages::escape_html(&book.title)
        ));
    }
    if entries.is_empty() {
        return String::new();
    }
    format!(
        "<section class=\"book-section-card book-related\" aria-labelledby=\"related-books-title\">         <p class=\"book-section-card__kicker\">Произведения</p>         <h2 class=\"book-section-card__title\" id=\"related-books-title\">Связанные книги</h2>         <ul class=\"book-related-grid\">{entries}</ul>         </section>"
    )
}

/// The containing public book was already checked by PgReader, but B repeats
/// the source and target visibility gates in SQL to close cross-book leaks.
pub(super) async fn attach(
    catalog: &PgCatalog,
    book_id: u64,
    likes: i64,
    html: Html<String>,
) -> Result<Html<String>, StatusCode> {
    let book_id_sql = i64::try_from(book_id).map_err(|_| StatusCode::NOT_FOUND)?;
    let related = catalog
        .public_related_books(book_id_sql)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let likes_html = pages::book_likes_strip(book_id, likes);
    Ok(Html(
        html.0
            .replace("<!--BOOK_LIKES-->", &likes_html)
            .replace("<!--BOOK_RELATED-->", &render(&related)),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s020_related_cards_escape_markup_and_skip_invalid_links() {
        let content = render(&[
            RelatedBookCard {
                id: 12,
                title: "<script>private?</script> & продолжение".to_owned(),
            },
            RelatedBookCard {
                id: -1,
                title: "Инвалид".to_owned(),
            },
        ]);
        assert!(content.contains("href=\"/books/12\""));
        assert!(content.contains("&lt;script&gt;private?&lt;/script&gt; &amp;"));
        assert!(!content.contains("<script>"));
        assert!(!content.contains("Инвалид"));
        assert_eq!(render(&[]), "");
    }

    #[test]
    fn s020_public_block_is_bounded_to_twelve_explicit_links() {
        let cards = (1..=15)
            .map(|id| RelatedBookCard {
                id,
                title: format!("Том {id}"),
            })
            .collect::<Vec<_>>();
        let html = render(&cards);
        assert_eq!(html.matches("book-related-grid__item").count(), 12);
        assert!(!html.contains("Том 13"));
    }
}
