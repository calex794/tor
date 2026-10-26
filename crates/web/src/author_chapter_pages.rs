//! Actual author book/chapter pages: no placeholder books or fabricated chapters.
use axum::response::Html;
use std::fmt::Write;
use tor_books::services::authoring::{ChapterAccess, OwnedBook, OwnedChapter, OwnedChapterSummary};

use crate::pages::{escape_html, layout};

pub fn detail(
    book: &OwnedBook,
    published: bool,
    chapters: &[OwnedChapterSummary],
    csrf: &str,
) -> Html<String> {
    let mut items = String::new();
    if chapters.is_empty() {
        items.push_str("<p class=\"empty-state\">В этой книге пока нет глав.</p>");
    } else {
        items.push_str("<ol>");
        for chapter in chapters {
            let access = match chapter.access {
                ChapterAccess::Free => "Бесплатная",
                ChapterAccess::Paid => "Платная",
            };
            let edit_action = if !published && chapter.access == ChapterAccess::Free {
                format!(
                    " <a href=\"/me/books/{}/chapters/{}/edit\">Редактировать</a>",
                    book.id, chapter.id
                )
            } else {
                String::new()
            };
            write!(
                &mut items,
                "<li>№{} — {} ({access}){edit_action}</li>",
                chapter.number,
                escape_html(&chapter.title)
            )
            .expect("write String");
        }
        items.push_str("</ol>");
        if chapters.len() == 200 {
            items.push_str("<p>Показаны первые 200 глав.</p>");
        }
    }

    let status = if published {
        "Опубликована"
    } else {
        "Черновик — книгу видит только владелец"
    };
    let action = if published {
        format!(
            "<p><a class=\"button button--quiet\" href=\"/books/{}\">Открыть опубликованную книгу</a></p>",
            book.id
        )
    } else if chapters.is_empty() {
        "<p>Сначала добавьте бесплатную главу, затем опубликуйте книгу.</p>".to_owned()
    } else {
        format!(
            r#"<form method="post" action="/me/books/{}/publish">
               <input type="hidden" name="csrf_token" value="{}">
               <button class="button" type="submit">Опубликовать книгу</button>
               </form>"#,
            book.id,
            escape_html(csrf)
        )
    };
    let add_action = if published {
        "<p>Новые главы для опубликованной книги требуют отдельного выпуска.</p>".to_owned()
    } else {
        format!(
            "<p><a class=\"button button--quiet\" href=\"/me/books/{}/chapters/new\">Добавить главу</a></p>",
            book.id
        )
    };
    let body = format!(
        r#"<section class="section auth-section"><div class="container"><div class="auth-card author-chapter-card">
        <p class="eyebrow">Кабинет автора · книга №{}</p>
        <h1>{}</h1>
        <p class="account-profile__hint">{status}</p>
        <h2>Главы</h2>{items}
        {add_action}
        {action}
        <p><a href="/me/books">Мои книги</a></p>
        </div></div></section>"#,
        book.id,
        escape_html(&book.title)
    );
    layout(&book.title, "Управление своей книгой", &body)
}

pub fn new_chapter(book_id: i64, csrf: &str, command_key: &str) -> Html<String> {
    let html = include_str!("../templates/author_new_chapter.html")
        .replace("<!--BOOK_ID-->", &book_id.to_string())
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf))
        .replace("<!--COMMAND_KEY-->", &escape_html(command_key));
    layout("Добавить главу", "Публикация главы автором", &html)
}

pub fn edit_chapter(
    book_id: i64,
    chapter_id: i64,
    chapter: &OwnedChapter,
    csrf: &str,
    command_key: &str,
) -> Html<String> {
    let html = include_str!("../templates/author_edit_chapter.html")
        .replace("<!--BOOK_ID-->", &book_id.to_string())
        .replace("<!--CHAPTER_ID-->", &chapter_id.to_string())
        .replace("<!--REVISION-->", &chapter.revision.to_string())
        .replace("<!--NUMBER-->", &chapter.number.to_string())
        .replace("<!--TITLE-->", &escape_html(&chapter.title))
        .replace("<!--BODY-->", &escape_html(&chapter.body))
        .replace("<!--CSRF_TOKEN-->", &escape_html(csrf))
        .replace("<!--COMMAND_KEY-->", &escape_html(command_key));
    layout("Редактировать главу", "Закрытый редактор черновика.", &html)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn author_title_is_always_escaped() {
        let page = detail(
            &OwnedBook {
                id: 42,
                title: "<img src=x onerror=1>".to_owned(),
            },
            false,
            &[],
            "test-token",
        );
        assert!(page.0.contains("&lt;img"));
        assert!(!page.0.contains("<img src=x"));
    }
    #[test]
    fn edit_form_escapes_title_and_textarea_breakout() {
        let chapter = OwnedChapter {
            number: 1,
            title: "<script>alert(1)</script>".to_owned(),
            body: "</textarea><script>evil()</script>".to_owned(),
            access: ChapterAccess::Free,
            price_amount: 0,
            revision: 7,
        };
        let page = edit_chapter(1, 3, &chapter, "token", "key");
        assert!(page.0.contains("value=\"7\""));
        assert!(page.0.contains("&lt;/textarea&gt;"));
        assert!(!page.0.contains("<script>evil()"));
    }
}
