//! Accessible, server-rendered private author pages with no script dependency.
#![forbid(unsafe_code)]

use crate::pages::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_books::services::authoring::OwnedBook;

fn private_layout(title: &str, description: &str, content: &str) -> Html<String> {
    let html = layout(title, description, content);
    Html(
        html.0
            .replace(
                "<a class=\"site-nav__link\" href=\"/login\">Войти</a>",
                "<a class=\"site-nav__link\" href=\"/me\">Мой кабинет</a>",
            )
            .replace(
                "<a class=\"button button--small\" href=\"/register\">Регистрация</a>",
                "<a class=\"button button--small\" href=\"/me/books\">Мои книги</a>",
            ),
    )
}

pub(crate) fn new_book(csrf: &str, command_key: &str) -> Html<String> {
    let content = include_str!("../templates/author_new_book.html")
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf))
        .replace("<!--COMMAND_KEY-->", &escape_html(command_key));
    private_layout("Создать книгу", "Новый черновик книги.", &content)
}

pub(crate) fn my_books(books: &[OwnedBook]) -> Html<String> {
    let mut cards = String::new();
    if books.is_empty() {
        cards.push_str(
            "<div class=\"library-empty\"><h2>Пока нет ваших книг</h2>\
             <p>Создайте черновик. Он будет доступен только вам, пока вы его не опубликуете.</p></div>",
        );
    } else {
        for book in books {
            write!(
                cards,
                "<article class=\"author-book\">\
                 <div><p class=\"eyebrow\">Личный раздел</p>\
                 <h2>{}</h2><p>Книга №{} · только автор</p></div>\
                 <a class=\"button button--quiet\" href=\"/author/books/{}\">Открыть черновик</a>\
                 </article>",
                escape_html(&book.title),
                book.id,
                book.id
            )
            .expect("write HTML");
        }
    }
    let html = format!(
        "<section class=\"section author-page\"><div class=\"container author-shell\">\
         <nav class=\"breadcrumbs\" aria-label=\"Навигационная цепочка\">\
         <a href=\"/me\">Мой кабинет</a><span aria-hidden=\"true\">/</span>\
         <span aria-current=\"page\">Мои книги</span></nav>\
         <div class=\"author-heading\"><div><p class=\"eyebrow\">Кабинет автора</p>\
         <h1>Мои книги</h1><p>Здесь видны ваши неопубликованные произведения.</p></div>\
         <a class=\"button\" href=\"/books/new\">Создать книгу</a></div>\
         <div class=\"author-books\">{cards}</div></div></section>"
    );
    private_layout("Мои книги", "Черновики автора.", &html)
}

pub(crate) fn owned_book(book: &OwnedBook) -> Html<String> {
    let html = format!(
        "<section class=\"section author-page\"><div class=\"container author-shell\">\
         <nav class=\"breadcrumbs\" aria-label=\"Навигационная цепочка\">\
         <a href=\"/me/books\">Мои книги</a><span aria-hidden=\"true\">/</span>\
         <span aria-current=\"page\">Черновик</span></nav>\
         <article class=\"author-detail\"><p class=\"eyebrow\">Черновик · книга №{}</p>\
         <h1>{}</h1><p>Книга сохранена в PostgreSQL и пока не видна читателям.</p>\
         <p>Добавление и выпуск глав подключаются следующим этапом.</p>\
         <a class=\"button button--quiet\" href=\"/me/books\">Вернуться к моим книгам</a>\
         </article></div></section>",
        book.id,
        escape_html(&book.title)
    );
    private_layout(&book.title, "Закрытый авторский черновик.", &html)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn author_books_escape_untrusted_titles() {
        let books = [OwnedBook {
            id: 14,
            title: "<script>alert(1)</script>".to_owned(),
        }];
        let html = my_books(&books).0;
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("/author/books/14"));
    }
}
