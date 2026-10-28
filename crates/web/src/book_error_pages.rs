//! Public NEwBOO book error pages with bounded, public-only recovery links.
//! Never use the missing book ID for promotion or expose hidden-book metadata.
use super::{pages, AppState};
use axum::{
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use std::{collections::HashSet, fmt::Write, time::Duration};
use tokio::sync::Semaphore;
use tor_search::{repositories::showcases::PgShowcases, services::showcases::RecentShowcases};

const MAX_PROMO_BOOKS: usize = 3;
const PROMO_QUERY_TIMEOUT: Duration = Duration::from_millis(180);
const PROMO_COVER_TIMEOUT: Duration = Duration::from_millis(70);
static PROMO_READS: Semaphore = Semaphore::const_new(2);

/// Load optional book links only on a missing book, never on a service outage.
/// Saturation, a slow database, or unavailable covers fall back to the plain
/// safe 404 page without turning a missing URL into unbounded database work.
pub(super) async fn public_book_error(state: &AppState, status: StatusCode) -> Response {
    if status != StatusCode::NOT_FOUND {
        return book_error(status);
    }
    let Some(catalog) = state.postgres.as_ref() else {
        return book_error(status);
    };
    let Ok(_permit) = PROMO_READS.try_acquire() else {
        return book_error(status);
    };
    let repository = PgShowcases::new(catalog.pool());
    let showcases = RecentShowcases::new(&repository);
    let Ok(Ok(books)) = tokio::time::timeout(
        PROMO_QUERY_TIMEOUT,
        showcases.latest_added(None, MAX_PROMO_BOOKS),
    )
    .await
    else {
        return book_error(status);
    };
    let cards = books
        .into_iter()
        .map(|book| pages::PublicBookCard {
            id: book.id,
            title: book.title,
            language: book.language,
            author_name: if book.author_name.trim().is_empty() {
                None
            } else {
                Some(book.author_name)
            },
            genres: Vec::new(),
        })
        .collect::<Vec<_>>();
    let covers = match tokio::time::timeout(
        PROMO_COVER_TIMEOUT,
        super::cover_http::cover_card_ids(state.covers.as_ref(), &cards),
    )
    .await
    {
        Ok(Ok(covers)) => covers,
        _ => HashSet::new(),
    };
    book_error_with_cards(status, &cards, &covers)
}

fn render_book_promos(cards: &[pages::PublicBookCard], covers: &HashSet<u64>) -> String {
    if cards.is_empty() {
        return String::new();
    }
    let mut markup = String::from(
        "<section class=\"book-error-promos\" aria-labelledby=\"book-error-promos-heading\">\
         <h2 id=\"book-error-promos-heading\">Другие книги на сайте</h2>\
         <div class=\"book-error-promos__grid\">",
    );
    for book in cards.iter().take(MAX_PROMO_BOOKS) {
        let title = pages::escape_html(&book.title);
        let author = book
            .author_name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .map(|name| {
                format!(
                    "<span class=\"book-error-promos__author\">{}</span>",
                    pages::escape_html(name)
                )
            })
            .unwrap_or_default();
        let cover = if covers.contains(&book.id) {
            format!(
                "<img class=\"book-error-promos__cover\" src=\"/covers/{}/card.webp\" alt=\"\" loading=\"lazy\" decoding=\"async\">",
                book.id
            )
        } else {
            String::from(
                "<span class=\"book-error-promos__cover book-error-promos__cover--empty\" aria-hidden=\"true\">Книга</span>",
            )
        };
        write!(
            markup,
            "<a class=\"book-error-promos__card\" href=\"/books/{id}\">{cover}\
             <span class=\"book-error-promos__body\">\
             <strong class=\"book-error-promos__title\">{title}</strong>{author}</span></a>",
            id = book.id,
        )
        .expect("write book promo");
    }
    markup.push_str("</div></section>");
    markup
}

/// Plain response is also used for 503, empty lists and bounded-query fallbacks.
pub(super) fn book_error(status: StatusCode) -> Response {
    book_error_with_cards(status, &[], &HashSet::new())
}

fn book_error_with_cards(
    status: StatusCode,
    cards: &[pages::PublicBookCard],
    covers: &HashSet<u64>,
) -> Response {
    let (code, title, message) = match status {
        StatusCode::NOT_FOUND => (
            "404",
            "Страница не найдена",
            "Возможно, страница была удалена, скрыта или ссылка устарела.",
        ),
        StatusCode::SERVICE_UNAVAILABLE => (
            "503",
            "Не удалось загрузить книгу",
            "Один из сервисов сейчас временно не отвечает. Попробуйте обновить страницу позже.",
        ),
        _ => return super::auth_cookies::private_response(status, &[]),
    };
    let promos = if status == StatusCode::NOT_FOUND {
        render_book_promos(cards, covers)
    } else {
        String::new()
    };
    let body = include_str!("../templates/book_error.html")
        .replace("<!--ERROR_CODE-->", code)
        .replace("<!--ERROR_HEADING-->", title)
        .replace("<!--ERROR_MESSAGE-->", message)
        .replace("<!--BOOK_PROMOS-->", &promos)
        .replace(
            "<!--RETRY_ACTION-->",
            if status == StatusCode::SERVICE_UNAVAILABLE {
                "<a class=\"not-found-card__button\" href=\"\">Попробовать ещё раз</a>"
            } else {
                ""
            },
        );
    let html = pages::layout(title, message, &body).0.replacen(
        "</head>",
        "<meta name=\"robots\" content=\"noindex, nofollow\">\n</head>",
        1,
    );
    let mut response = super::auth_cookies::private_response(
        (status, axum::response::Html(html)).into_response(),
        &[],
    );
    if status == StatusCode::SERVICE_UNAVAILABLE {
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("30"));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(id: u64, title: &str, author: Option<&str>) -> pages::PublicBookCard {
        pages::PublicBookCard {
            id,
            title: title.to_owned(),
            language: "ru".into(),
            author_name: author.map(str::to_owned),
            genres: Vec::new(),
        }
    }

    #[tokio::test]
    async fn h_11_book_error_404_is_complete_private_and_uncacheable() {
        let response = book_error(StatusCode::NOT_FOUND);
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert!(!response.headers().contains_key(header::RETRY_AFTER));
        assert!(response.headers().contains_key("content-security-policy"));
        let html = axum::body::to_bytes(response.into_body(), 65_536)
            .await
            .unwrap();
        let html = String::from_utf8(html.to_vec()).unwrap();
        assert!(html.contains("lang=\"ru\""));
        assert!(html.contains("not-found-card"));
        assert!(html.contains("Страница не найдена"));
        assert!(html.contains("noindex, nofollow"));
        assert!(html.contains("href=\"/catalog\""));
        assert!(!html.contains("book_id"));
        assert!(!html.contains("href=\"\""));
    }

    #[tokio::test]
    async fn h_11_public_book_promotion_escapes_html_and_verifies_covers() {
        let books = vec![
            card(12, "<script>Название</script>", Some("\" onclick=\"bad")),
            card(9, "Без обложки", None),
        ];
        let response = book_error_with_cards(
            StatusCode::NOT_FOUND,
            &books,
            &HashSet::from([12_u64]),
        );
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = axum::body::to_bytes(response.into_body(), 65_536)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("Другие книги на сайте"));
        assert!(html.contains("href=\"/books/12\""));
        assert!(html.contains("src=\"/covers/12/card.webp\""));
        assert!(!html.contains("/covers/9/card.webp"));
        assert!(html.contains("&lt;script&gt;Название&lt;/script&gt;"));
        assert!(!html.contains("<script>Название</script>"));
        assert!(html.contains("&quot; onclick=&quot;bad"));
        assert!(!html.contains("onclick=\"bad"));
        assert!(html.contains("noindex, nofollow"));
    }

    #[tokio::test]
    async fn h_11_503_never_promotes_and_404_limits_to_three_books() {
        let books = (1..=9)
            .map(|id| card(id, "Тест", None))
            .collect::<Vec<_>>();
        let covers = HashSet::new();
        let down = book_error_with_cards(StatusCode::SERVICE_UNAVAILABLE, &books, &covers);
        assert_eq!(down.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(down.headers()[header::RETRY_AFTER], "30");
        assert_eq!(down.headers()[header::CACHE_CONTROL], "no-store");
        let body = axum::body::to_bytes(down.into_body(), 65_536)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(!html.contains("book-error-promos"));
        let missing = book_error_with_cards(StatusCode::NOT_FOUND, &books, &covers);
        let body = axum::body::to_bytes(missing.into_body(), 65_536)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(html.matches("class=\"book-error-promos__card\"").count(), 3);
    }
}
