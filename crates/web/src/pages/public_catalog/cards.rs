use super::{escape_html, PublicBookCard};
use std::{collections::HashSet, fmt::Write};
use tor_search::domain::seo_urls::BookUrl;

/// NEwBOO BookCard layout, with real PostgreSQL-backed author and genre metadata.
pub(super) fn book_cards(
    books: &[PublicBookCard],
    shelf: bool,
    shelf_label: &str,
    covers: &HashSet<u64>,
    observer: super::Observer<'_>,
) -> String {
    if books.is_empty() {
        return "<div class=\"ui-empty-state\"><h3 class=\"ui-empty-state__title\">Каталог пока наполняется</h3><p class=\"ui-empty-state__text\">В каталоге пока нет опубликованных книг.</p></div>".to_owned();
    }
    let mut html = if shelf {
        format!(
            "<div class=\"home-shelf__track\" aria-label=\"{}\">",
            escape_html(shelf_label)
        )
    } else {
        String::from("<div class=\"catalog-grid\">")
    };
    for book in books {
        let Some(url) = BookUrl::from_id(book.id) else {
            continue;
        };
        let href = url.path();
        let title = escape_html(&book.title);
        let mut details = String::new();
        // Match the original NEwBOO BookCard: public author if present, not
        // invented likes/comments or an extra language/genre metadata block.
        if let Some(author) = book
            .author_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
        {
            write!(
                details,
                "<div class=\"book-mini-card__meta\">{}</div>",
                escape_html(author),
            )
            .expect("format String");
        }
        let variant = if shelf {
            "book-mini-card--shelf"
        } else {
            "book-mini-card--compact"
        };
        let cover = if covers.contains(&book.id) {
            format!(
                r#"<img src="/covers/{}/card.webp" alt="" loading="lazy" decoding="async" draggable="false">"#,
                book.id
            )
        } else {
            "<span>Нет обложки</span>".to_owned()
        };
        let observation = observer
            .map(|(issuer, surface)| {
                crate::flow_exposure::attributes(
                    Some(issuer),
                    tor_contracts::exposure::CardObservation {
                        book_id: book.id as i64,
                        surface,
                        editorial: None,
                    },
                )
            })
            .unwrap_or_default();
        write!(
            html,
            "<article class=\"book-mini-card {variant}\"{observation}><a data-flow-link class=\"book-mini-card__link\" href=\"{href}\" aria-label=\"{title}\"><div class=\"book-mini-card__cover book-shimmer\">{cover}</div><div class=\"book-mini-card__body\"><h3 class=\"book-mini-card__title\">{title}</h3>{details}</div></a></article>",
        )
        .expect("format String");
    }
    html.push_str("</div>");
    html
}
