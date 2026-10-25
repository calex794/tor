//! Server-rendered demo pages based on BookaRust source d11b27b43125b93e59809147e8e687203fa0753b.
use axum::{
    http::header,
    response::{Html, IntoResponse},
};
use std::fmt::Write;

#[derive(Clone, Debug)]
pub struct PublicBookCard {
    pub id: u64,
    pub title: String,
    pub language: String,
}

pub fn escape_html(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

pub fn layout(title: &str, description: &str, content: &str) -> Html<String> {
    Html(
        include_str!("../templates/layout.html")
            .replace("<!--PAGE_TITLE-->", &escape_html(title))
            .replace("<!--PAGE_DESCRIPTION-->", &escape_html(description))
            .replace("<!--PAGE_CONTENT-->", content),
    )
}

fn book_cards(books: &[PublicBookCard]) -> String {
    if books.is_empty() {
        return "<p class=\"empty-state\">В каталоге пока нет опубликованных книг.</p>".to_owned();
    }
    let mut html = String::new();
    for book in books {
        let title = escape_html(&book.title);
        let language = escape_html(&book.language);
        write!(html,
            "<article class=\"book-card\"><div class=\"book-card__cover\" aria-hidden=\"true\">К</div><div class=\"book-card__body\"><span class=\"book-card__label\">Книга · {language}</span><h3 class=\"book-card__title\"><a href=\"/books/{}\">{title}</a></h3></div></article>",
            book.id).expect("format String");
    }
    html
}

pub fn home(books: &[PublicBookCard]) -> Html<String> {
    let content =
        include_str!("../templates/home.html").replace("<!--PUBLIC_BOOKS-->", &book_cards(books));
    layout(
        "Главная",
        "Пандаолейт — демонстрационный каталог без пользовательских данных.",
        &content,
    )
}

pub fn catalog(books: &[PublicBookCard]) -> Html<String> {
    let content = include_str!("../templates/catalog.html")
        .replace("<!--PUBLIC_BOOKS-->", &book_cards(books));
    layout(
        "Каталог",
        "Опубликованные книги и переводы на Пандаолейт.",
        &content,
    )
}

pub fn book_detail(book: &PublicBookCard) -> Html<String> {
    let title = escape_html(&book.title);
    let language = escape_html(&book.language);
    let body = format!(
        "<section class=\"section book-detail-page\"><div class=\"container\">         <nav class=\"breadcrumbs\" aria-label=\"Навигационная цепочка\">         <a href=\"/\">Главная</a><span aria-hidden=\"true\">/</span>         <a href=\"/catalog\">Каталог</a><span aria-hidden=\"true\">/</span>         <span aria-current=\"page\">{title}</span></nav>         <div class=\"book-detail__layout\">         <div class=\"book-detail__art\" aria-hidden=\"true\"><span>П</span></div>         <div class=\"book-detail__info\">         <p class=\"eyebrow\">Книга · {language}</p><h1>{title}</h1>         <p class=\"book-detail__description\">Карточка опубликованной книги.         Оглавление и чтение глав станут доступны после подключения читалки.</p>         <div class=\"book-detail__actions\">         <a class=\"button\" href=\"/catalog\">Вернуться к книгам</a>         <span class=\"book-detail__note\">Вход для просмотра каталога не требуется</span>         </div></div></div></div></section>"
    );
    layout(&book.title, "Карточка книги в каталоге Пандаолейт.", &body)
}

pub async fn stylesheet() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/css; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        include_str!("../assets/site.css"),
    )
}

pub fn inactive_account_page(title: &str) -> impl IntoResponse {
    ([(header::CACHE_CONTROL, "no-store")],
     layout(title, "Аккаунты пока недоступны.", "<section class=\"section auth-section\"><div class=\"container\"><div class=\"auth-card\"><p class=\"eyebrow\">Профиль читателя</p><h1>Аккаунты пока не открыты</h1><p class=\"page-description\">Вход и регистрация будут доступны после завершения проверки безопасности и подключения постоянной базы данных. Сейчас каталог можно просматривать без аккаунта.</p><a class=\"button\" href=\"/catalog\">Перейти в каталог</a></div></div></section>"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escapes_untrusted_book_titles() {
        let page = home(&[PublicBookCard {
            id: 9,
            title: "<script>alert('x')</script> &".to_owned(),
            language: "ru".into(),
        }]);
        assert!(page.0.contains("&lt;script&gt;"));
        assert!(!page.0.contains("<script>"));
        assert!(page.0.contains("href=\"/books/9\""));
    }
    #[test]
    fn empty_catalog_is_explicit() {
        assert!(catalog(&[])
            .0
            .contains("В каталоге пока нет опубликованных книг"));
    }
}
