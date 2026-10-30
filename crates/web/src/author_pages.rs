//! Private author book pages. No fake books and no client-supplied user IDs.
use axum::response::Html;
use std::fmt::Write;
use tor_books::services::authoring::OwnedBook;

use crate::pages::{escape_html, layout};

pub fn index(books: &[OwnedBook]) -> Html<String> {
    let mut list = String::new();
    if books.is_empty() {
        list.push_str("<p class=\"empty-state\">У вас пока нет книг. Создайте первую.</p>");
    } else {
        list.push_str("<ul class=\"author-book-list\">");
        for book in books {
            write!(
                &mut list,
                "<li><a class=\"author-book-link\" href=\"/me/books/{}\">{}</a></li>",
                book.id,
                escape_html(&book.title)
            )
            .expect("write String");
        }
        list.push_str("</ul>");
        if books.len() == 100 {
            list.push_str("<p>Показаны последние 100 книг.</p>");
        }
    }
    let content = format!(
        r#"<section class="section auth-section"><div class="container"><div class="auth-card author-card">
        <p class="eyebrow">Кабинет автора</p><h1>Мои книги</h1>
        <p><a class="button" href="/me/books/new">Создать книгу</a></p>
        {list}
        <p><a href="/me">Вернуться в кабинет</a></p>
        </div></div></section>"#
    );
    layout("Мои книги", "Ваши книги из PostgreSQL.", &content)
}

pub fn new_book(csrf: &str, command_key: &str) -> Html<String> {
    let content = include_str!("../templates/author_new_book.html")
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf))
        .replace("<!--COMMAND_KEY-->", &escape_html(command_key))
        .replace("<!--BOOK_TITLE-->", "")
        .replace("<!--BOOK_ORIGINAL-->", "")
        .replace("<!--BOOK_ALTERNATIVES-->", "")
        .replace("<!--BOOK_AUTHOR-->", "")
        .replace("<!--BOOK_DESCRIPTION-->", "")
        .replace("<!--BOOK_GENRES-->", "")
        .replace("<!--BOOK_TAGS-->", "")
        .replace("<!--BOOK_FANDOMS-->", "")
        .replace("<!--BOOK_LANGUAGE-->", "ru")
        .replace("<!--BOOK_DUPLICATE_RESULTS-->", "")
        .replace("<!--BOOK_CREATE_BUTTON-->", "");
    layout("Создать книгу", "Новая книга в вашем кабинете.", &content)
}

pub fn review_new_book(
    form: &crate::auth::NewBookForm,
    candidates: &[tor_books::services::title_duplicates::TitleCandidate],
) -> Html<String> {
    let mut form_html = include_str!("../templates/author_new_book.html").to_owned();
    let fields = [
        ("<!--CSRF_TOKEN-->", form.csrf_token.as_deref().unwrap_or("")),
        ("<!--COMMAND_KEY-->", form.command_key.as_str()),
        ("<!--BOOK_TITLE-->", form.title.as_str()),
        ("<!--BOOK_ORIGINAL-->", form.original_title.as_deref().unwrap_or("")),
        ("<!--BOOK_ALTERNATIVES-->", form.alternative_titles.as_deref().unwrap_or("")),
        ("<!--BOOK_AUTHOR-->", form.author_name.as_deref().unwrap_or("")),
        ("<!--BOOK_DESCRIPTION-->", form.description.as_deref().unwrap_or("")),
        ("<!--BOOK_GENRES-->", form.genres.as_deref().unwrap_or("")),
        ("<!--BOOK_TAGS-->", form.tags.as_deref().unwrap_or("")),
        ("<!--BOOK_FANDOMS-->", form.fandoms.as_deref().unwrap_or("")),
        ("<!--BOOK_LANGUAGE-->", form.target_language.as_str()),
    ];
    for (marker, value) in fields {
        form_html = form_html.replace(marker, &escape_html(value));
    }
    let mut list = String::new();
    if candidates.is_empty() {
        list.push_str("<p>Похожих доступных книг не найдено. Это не гарантия отсутствия дубля.</p>");
    } else {
        list.push_str("<p>Проверьте найденные книги: похожее название не обязательно означает одно произведение.</p><ul>");
        for candidate in candidates {
            let match_kind = if candidate.exact { "совпадение названия" } else { "похожее название" };
            write!(
                &mut list,
                "<li><a href=\"/books/{}\">{}</a> (язык: {}; {})</li>",
                candidate.id,
                escape_html(&candidate.title),
                escape_html(&candidate.target_language),
                match_kind,
            )
            .expect("write String");
        }
        list.push_str("</ul>");
    }
    let review = format!("<aside aria-live=\"polite\"><h2>Возможные совпадения</h2>{list}<p>Если это другое произведение или перевод, можно продолжить создание.</p></aside>");
    form_html = form_html.replace("<!--BOOK_DUPLICATE_RESULTS-->", &review);
    form_html = form_html.replace(
        "<!--BOOK_CREATE_BUTTON-->",
        "<button class=\"button\" type=\"submit\" formaction=\"/me/books\">Создать книгу после проверки</button>",
    );
    layout("Проверка названия книги", "Перед созданием проверьте возможные совпадения.", &form_html)
}

#[cfg(test)]
mod duplicate_pages_tests {
    use super::*;

    #[test]
    fn review_form_escapes_values_and_preserves_command_key() {
        let form = crate::auth::NewBookForm {
            title: "<script>alert(1)</script>".into(),
            original_title: None,
            alternative_titles: None,
            author_name: Some("Test".into()),
            description: None,
            genres: None,
            tags: None,
            fandoms: None,
            target_language: "ru".into(),
            command_key: "k".repeat(43),
            csrf_token: Some("c".repeat(43)),
        };
        let html = review_new_book(&form, &[]).0;
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>alert"));
        assert!(html.contains("Создать книгу после проверки"));
        assert!(html.contains(&"k".repeat(43)));
        assert!(html.contains("name=\"original_title\""));
    }
}
