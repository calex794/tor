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
        r#"<section class="section auth-section"><div class="container"><div class="auth-card">
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

pub fn detail(book: &OwnedBook) -> Html<String> {
    let content = format!(
        r#"<section class="section auth-section"><div class="container"><div class="auth-card">
        <p class="eyebrow">Кабинет автора · книга №{}</p>
        <h1>{}</h1>
        <p>Книга сохранена в PostgreSQL и доступна только вам до публикации.</p>
        <p><a class="button button--quiet" href="/me/books">Мои книги</a></p>
        </div></div></section>"#,
        book.id,
        escape_html(&book.title)
    );
    layout(&book.title, "Частная страница книги автора.", &content)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_book_title_is_html_escaped() {
        let page = detail(&OwnedBook {
            id: 37,
            title: "<script>alert(1)</script>".to_owned(),
        });
        assert!(page.0.contains("&lt;script&gt;"));
        assert!(!page.0.contains("<script>"));
    }
}
