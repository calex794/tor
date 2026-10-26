//! The original NEwBOO public catalogue layout, sourced from real TOR read data.
use super::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;

#[derive(Clone, Debug)]
pub struct PublicBookCard {
    pub id: u64,
    pub title: String,
    pub language: String,
    pub author_name: Option<String>,
    pub genres: Vec<String>,
}

/// NEwBOO BookCard layout, with real PostgreSQL-backed author and genre metadata.
fn book_cards(books: &[PublicBookCard]) -> String {
    if books.is_empty() {
        return "<div class=\"ui-empty-state\"><h3 class=\"ui-empty-state__title\">Каталог пока наполняется</h3><p class=\"ui-empty-state__text\">В каталоге пока нет опубликованных книг.</p></div>".to_owned();
    }
    let mut html = String::from("<div class=\"catalog-grid\">");
    for book in books {
        let title = escape_html(&book.title);
        let language = escape_html(&book.language);
        let mut details = String::new();
        if let Some(author) = &book.author_name {
            write!(
                details,
                "<p class=\"book-card__meta book-mini-card__meta\">Автор: {}</p>",
                escape_html(author),
            )
            .expect("format String");
        }
        if !book.genres.is_empty() {
            write!(
                details,
                "<p class=\"book-card__meta book-mini-card__meta\">Жанры: {}</p>",
                escape_html(&book.genres.join(", ")),
            )
            .expect("format String");
        }
        write!(
            html,
            "<article class=\"book-mini-card book-mini-card--compact\"><a class=\"book-mini-card__link\" href=\"/books/{}\" aria-label=\"{title}\"><div class=\"book-mini-card__cover\"><span>Нет обложки</span></div><div class=\"book-mini-card__body\"><h3 class=\"book-mini-card__title\">{title}</h3><p class=\"book-mini-card__meta\">Язык: {language}</p>{details}</div></a></article>",
            book.id,
        )
        .expect("format String");
    }
    html.push_str("</div>");
    html
}

pub fn home(books: &[PublicBookCard]) -> Html<String> {
    let content =
        include_str!("../../templates/home.html").replace("<!--PUBLIC_BOOKS-->", &book_cards(books));
    layout(
        "Главная",
        "Пандаолейт — книги и переводы для чтения онлайн.",
        &content,
    )
}

pub fn catalog(books: &[PublicBookCard]) -> Html<String> {
    let content = include_str!("../../templates/catalog.html")
        .replace("<!--PUBLIC_BOOKS-->", &book_cards(books))
        .replace("<!--SEARCH_FORM-->", "");
    layout(
        "Каталог",
        "Опубликованные книги и переводы на Пандаолейт.",
        &content,
    )
}

fn encode_search_term(value: &str) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~".contains(&byte) {
            result.push(char::from(byte));
        } else {
            write!(result, "%{byte:02X}").expect("string write");
        }
    }
    result
}

pub fn catalog_search(
    books: &[PublicBookCard],
    query: &str,
    language: Option<&str>,
    genre: Option<&str>,
    cursor: Option<&str>,
) -> Html<String> {
    let mut controls = format!(
        r#"<form class="catalog-filters" action="/catalog" method="get" role="search" aria-label="Фильтры каталога"><input class="ui-input catalog-filters__search" id="book-query" type="search" name="q" maxlength="80" value="{}" placeholder="Название или автор" aria-label="Название или автор"><input class="ui-input" id="book-genre" type="text" name="genre" maxlength="40" value="{}" placeholder="Жанр (фэнтези)" aria-label="Жанр"><input class="ui-input catalog-filters__select" id="book-language" name="language" maxlength="32" placeholder="Язык перевода (ru)" value="{}" aria-label="Язык перевода"><button class="ui-button ui-button--primary catalog-filters__submit" type="submit">Найти</button></form>"#,
        escape_html(query),
        escape_html(genre.unwrap_or("")),
        escape_html(language.unwrap_or("")),
    );
    if let Some(cursor) = cursor {
        let language_param = language
            .filter(|value| !value.is_empty())
            .map(|value| format!("&amp;language={}", encode_search_term(value)))
            .unwrap_or_default();
        let genre_param = genre
            .filter(|value| !value.is_empty())
            .map(|value| format!("&amp;genre={}", encode_search_term(value)))
            .unwrap_or_default();
        write!(
            controls,
            r#"<nav class="catalog-pagination" aria-label="Следующая страница поиска"><a class="ui-button ui-button--ghost" href="/catalog?q={}{}{}&amp;after={}">Показать ещё</a></nav>"#,
            encode_search_term(query),
            language_param,
            genre_param,
            escape_html(cursor),
        ).expect("string write");
    }
    let content = include_str!("../../templates/catalog.html")
        .replace("<!--PUBLIC_BOOKS-->", &book_cards(books))
        .replace("<!--SEARCH_FORM-->", &controls);
    layout(
        "Каталог",
        "Поиск опубликованных книг на Пандаолейт.",
        &content,
    )
}

