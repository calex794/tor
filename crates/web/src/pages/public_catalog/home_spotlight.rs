//! NEwBOO-style spotlight built only from public recent books.
//! Not an ad, daily top, or personalized recommendation.
use super::{escape_html, PublicBookCard};
use std::{collections::HashSet, fmt::Write};
use tor_search::domain::seo_urls::BookUrl;

const MAX_SPOTLIGHT_BOOKS: usize = 5;

pub(super) fn render(books: &[PublicBookCard], covers: &HashSet<u64>) -> String {
    let mut html = String::from(
        "<div class=\"home-special-carousel\" role=\"region\" aria-label=\"Новые книги\">\
        <div class=\"home-special-carousel__viewport\" tabindex=\"0\" \
        aria-label=\"Листайте книги горизонтально\">\
        <div class=\"home-special-carousel__track\">",
    );
    for book in books.iter().take(MAX_SPOTLIGHT_BOOKS) {
        let Some(url) = BookUrl::from_id(book.id) else {
            continue;
        };
        let href = url.path();
        let title = escape_html(&book.title);
        let details = book
            .author_name
            .as_deref()
            .filter(|author| !author.trim().is_empty())
            .map(escape_html)
            .unwrap_or_else(|| format!("Язык: {}", escape_html(&book.language)));
        // Only callers' verified same-origin cover IDs are rendered.
        let bg = if covers.contains(&book.id) {
            format!(
                r#"<img class="home-spotlight-card__bg" src="/covers/{}/main.webp" alt="" aria-hidden="true" loading="lazy" decoding="async" draggable="false">"#,
                book.id
            )
        } else {
            String::new()
        };
        let cover = if covers.contains(&book.id) {
            format!(
                r#"<img class="home-spotlight-card__cover" src="/covers/{}/main.webp" alt="" loading="lazy" decoding="async" draggable="false">"#,
                book.id
            )
        } else {
            String::from(
                r#"<span class="home-spotlight-card__cover home-spotlight-card__cover--empty">Нет обложки</span>"#,
            )
        };
        write!(
            html,
            r#"<a class="home-spotlight-card" href="{href}" aria-label="Открыть книгу {title}">
               {bg}<div class="home-spotlight-card__inner">
               <div class="home-spotlight-card__content">
               <span class="home-spotlight-card__eyebrow">Недавно добавлена</span>
               <h2 class="home-spotlight-card__title">{title}</h2>
               <p class="home-spotlight-card__text">{details}</p></div>
               <div class="home-spotlight-card__cover-wrap">{cover}</div>
               </div></a>"#,
        )
        .expect("write spotlight");
    }
    html.push_str("</div></div>");
    if books.len() > 1 {
        html.push_str(
            "<p class=\"home-special-carousel__hint\">Листайте, чтобы увидеть другие книги</p>",
        );
    }
    html.push_str("</div>");
    html
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(id: u64, title: &str) -> PublicBookCard {
        PublicBookCard {
            id,
            title: title.to_owned(),
            language: "ru".into(),
            author_name: None,
            genres: Vec::new(),
        }
    }

    #[test]
    fn h_10_spotlight_limits_items_without_fake_top() {
        let books: Vec<_> = (1..=7)
            .rev()
            .map(|id| book(id, &format!("Книга {id}")))
            .collect();
        let html = render(&books, &HashSet::new());
        assert_eq!(html.matches("class=\"home-spotlight-card\"").count(), 5);
        assert!(html.contains("href=\"/books/7\""));
        assert!(!html.contains("href=\"/books/2\""));
        assert!(!html.contains("Топ дня"));
        assert!(!html.contains("Рекомендуем для вас"));
    }

    #[test]
    fn h_10_spotlight_does_not_leak_unverified_covers() {
        let html = render(
            &[book(3, "Есть обложка"), book(2, "Без обложки")],
            &HashSet::from([3]),
        );
        assert_eq!(html.matches(r#"src="/covers/3/main.webp""#).count(), 2);
        assert!(!html.contains("/covers/2/"));
        assert!(html.contains("Нет обложки"));
        assert!(html.contains("home-special-carousel__viewport"));
    }

    #[test]
    fn h_11_spotlight_escapes_untrusted_metadata() {
        let mut malicious = book(4, "<script>x</script>");
        malicious.author_name = Some("\" onmouseover=\"evil".into());
        let html = render(&[malicious], &HashSet::new());
        assert!(!html.contains("<script>"));
        assert!(!html.contains("onmouseover=\"evil"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&quot; onmouseover=&quot;evil"));
        assert!(!html.contains("home-special-carousel__hint"));
    }
}
