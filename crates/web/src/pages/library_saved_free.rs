//! Private saved-free editions: archive-like read-only view, not public reader.
use super::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_reading::services::library::{SavedFreeChapter, SavedFreeChapterPage};

pub fn saved_free_list(book_id: i64, page: &SavedFreeChapterPage) -> Html<String> {
    let mut rows = String::new();
    for item in &page.items {
        let title = escape_html(&item.chapter_title);
        write!(
            rows,
            "<li><a href=\"/me/library/saved/{}\">Глава {}. {title}</a> \
             <span class=\"library-card__place\">Сохранённая редакция {}</span></li>",
            item.chapter_id, item.chapter_no, item.version_no
        )
        .expect("write saved free link");
    }
    if rows.is_empty() {
        rows.push_str("<li>Сохранённых бесплатных глав пока нет.</li>");
    }
    let next = page
        .next_cursor
        .map(|id| {
            format!(
                "<p><a href=\"/me/library/{book_id}/saved?after={id}\">Следующие главы</a></p>"
            )
        })
        .unwrap_or_default();
    let content = format!(
        "<section class=\"section auth-section\"><div class=\"container\">\
         <h1>Сохранённые бесплатные главы</h1>\
         <p>Здесь доступны только ранее закреплённые редакции. \
         Новые главы и платные тексты автоматически не открываются.</p>\
         <ul>{rows}</ul>{next}\
         <p><a href=\"/me/library\">Вернуться в библиотеку</a></p>\
         </div></section>"
    );
    layout("Сохранённые главы", "Личная библиотека, сохранённые редакции.", &content)
}

pub fn saved_free_reader(chapter: &SavedFreeChapter) -> Html<String> {
    let body = tor_reading::domain::renderer::render(&chapter.body_markdown);
    let title = escape_html(&chapter.chapter_title);
    let book = escape_html(&chapter.book_title);
    let content = format!(
        "<section class=\"section reader-page\"><div class=\"container\">\
         <p class=\"eyebrow\">Сохранённая редакция {}</p>\
         <h1>{book}: глава {}. {title}</h1>\
         <p>Текст закреплён в личной библиотеке. Текущая версия книги может отличаться.</p>\
         <article class=\"reader-content\">{body}</article>\
         <p><a href=\"/me/library/{}/saved\">К сохранённым главам</a></p>\
         </div></section>",
        chapter.version_no, chapter.chapter_no, chapter.book_id
    );
    layout(&chapter.chapter_title, "Ранее сохранённая бесплатная редакция.", &content)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archived_free_text_uses_sanitized_renderer_and_escapes_titles() {
        let saved = SavedFreeChapter {
            book_id: 1,
            book_title: "<script>alert(1)</script>".into(),
            chapter_id: 6,
            chapter_no: 1,
            chapter_title: "X & Y".into(),
            version_id: 10,
            version_no: 2,
            body_markdown: "**Original V1**".into(),
        };
        let html = saved_free_reader(&saved).0;
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("X &amp; Y"));
        assert!(html.contains("Original V1"));
        assert!(html.contains("/me/library/1/saved"));
    }
}
