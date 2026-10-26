//! Actual author book/chapter pages: no placeholder books or fabricated chapters.
use axum::response::Html;
use std::fmt::Write;
use tor_books::services::authoring::{
    ChapterAccess, OwnedBook, OwnedChapter, OwnedChapterSummary, PendingChapter,
};

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
            let pending = published && !chapter.published;
            let edit_action = if chapter.access == ChapterAccess::Free && (!published || pending) {
                format!(
                    " <a href=\"/me/books/{}/chapters/{}/edit\">Редактировать</a>",
                    book.id, chapter.id
                )
            } else {
                String::new()
            };
            let release_action = if pending && chapter.access == ChapterAccess::Free {
                format!(
                    " <a href=\"/me/books/{}/chapters/{}/release\">Выпустить главу</a>",
                    book.id, chapter.id
                )
            } else {
                String::new()
            };
            let state = if pending { " — Черновик (не виден читателям)" } else { "" };
            write!(
                &mut items,
                "<li>№{} — {} ({access}){state}{edit_action}{release_action}</li>",
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
    let add_action = format!(
        "<p><a class=\"button button--quiet\" href=\"/me/books/{}/chapters/new\">Добавить главу</a></p>",
        book.id
    );
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

/// Confirmation is a separate page; the text and immutable version are shown
/// to the owner immediately before an explicit publication POST.
pub fn release_chapter(
    book_id: i64,
    chapter_id: i64,
    chapter: &PendingChapter,
    csrf: &str,
) -> Html<String> {
    let content = format!(
        r#"<section class="section auth-section"><div class="container">
        <div class="auth-card author-chapter-card">
        <p class="eyebrow">Кабинет автора · книга №{book_id}</p>
        <h1>Выпустить главу №{}</h1>
        <h2>{}</h2>
        <p>Проверяется редакция №{} (версия №{}). После выпуска эту версию смогут читать все.</p>
        <pre class="author-release-preview">{}</pre>
        <form method="post" action="/me/books/{book_id}/chapters/{chapter_id}/release">
          <input type="hidden" name="csrf_token" value="{}">
          <input type="hidden" name="expected_revision" value="{}">
          <input type="hidden" name="version_id" value="{}">
          <button class="button" type="submit">Опубликовать эту главу</button>
        </form>
        <p><a href="/me/books/{book_id}">Вернуться к черновикам</a></p>
        </div></div></section>"#,
        chapter.number,
        escape_html(&chapter.title),
        chapter.revision,
        chapter.version_id,
        escape_html(&chapter.body),
        escape_html(csrf),
        chapter.revision,
        chapter.version_id,
    );
    layout("Выпустить главу", "Подтверждение публикации выбранной версии.", &content)
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
    fn release_form_escapes_chapter_body_and_pins_version() {
        let chapter = PendingChapter {
            number: 4,
            title: "<svg onload=1>".into(),
            body: "</pre><script>bad()</script>".into(),
            revision: 3,
            version_id: 86,
        };
        let page = release_chapter(2, 5, &chapter, "token");
        assert!(page.0.contains("name=\"version_id\" value=\"86\""));
        assert!(page.0.contains("name=\"expected_revision\" value=\"3\""));
        assert!(page.0.contains("&lt;/pre&gt;"));
        assert!(!page.0.contains("<script>bad()"));
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
