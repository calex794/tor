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
                "<li><a class=\"author-book-link\" href=\"/me/books/{}\">{}</a> <a href=\"/me/books/{}/recommendations\">Рекомендации</a></li>",
                book.id,
                escape_html(&book.title),
                book.id
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
        .replace("<!--COMMAND_KEY-->", &escape_html(command_key));
    layout("Создать книгу", "Новая книга в вашем кабинете.", &content)
}

/// Generic duplicate response. Do not reveal whether another user's draft exists.
pub fn create_conflict() -> Html<String> {
    let content = r#"<section class="section auth-section">
      <div class="container"><div class="auth-card author-card" role="alert">
        <p class="eyebrow">Регистрация книги</p>
        <h1>Книга не создана</h1>
        <p>Такое название или ссылка на оригинал уже может использоваться другой книгой.
        Проверьте каталог. Если это ваша книга, откройте её в личном кабинете.
        Также проверьте, не меняли ли вы данные при повторной отправке формы.</p>
        <p><a class="button" href="/catalog">Проверить каталог</a></p>
        <p><a href="/me/books">Мои книги</a> ·
        <a href="/me/books/new">Попробовать с другим названием</a></p>
      </div></div>
    </section>"#;
    layout(
        "Книга не создана",
        "Обнаружено возможное совпадение книги.",
        content,
    )
}
