//! Private server-rendered administrator preview; no buyer grant or wallet event.
use super::escape_html;
use axum::response::Html;
use tor_books::repositories::admin_reading::AdministratorChapter;

pub fn administrator_chapter_view(chapter: &AdministratorChapter) -> Html<String> {
    let title = escape_html(&chapter.chapter_title);
    let book = escape_html(&chapter.book_title);
    let access = escape_html(&chapter.access_level);
    let version_id = chapter.version_id;
    let version = chapter.version_no;
    // The existing reader renderer escapes author-provided HTML and unsafe URLs.
    let rendered = tor_reading::domain::renderer::render(&chapter.body_markdown);
    let content = format!(
        r#"<section class="section reader-page reader-private" data-administrator-reader
             data-version-id="{version_id}" aria-labelledby="reader-title">
            <div class="container reader-width">
              <header class="reader-topbar">
                <span>Административное чтение — {book}</span>
              </header>
              <p class="eyebrow">Администратор · {access} · Редакция {version}</p>
              <h1 id="reader-title">{title}</h1>
              <article class="chapter-content reader-markdown">{rendered}</article>
            </div>
          </section>"#,
    );
    Html(
        include_str!("../../templates/reader_layout.html")
            .replace("<!--PAGE_TITLE-->", &title)
            .replace("<!--PAGE_DESCRIPTION-->", "Административное чтение")
            .replace("<!--PAGE_CONTENT-->", &content),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn administrator_preview_escapes_untrusted_markup() {
        let chapter = AdministratorChapter {
            book_title: "<script>alert(1)</script>".into(),
            chapter_title: "<img src=x>".into(),
            version_id: 14,
            version_no: 2,
            access_level: "paid".into(),
            body_markdown: "Скрытый текст\n\n<script>alert(2)</script>".into(),
        };
        let html = administrator_chapter_view(&chapter).0;
        assert!(html.contains("Скрытый текст"));
        assert!(html.contains("data-administrator-reader"));
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(!html.contains("<script>alert(2)</script>"));
        assert!(!html.contains("<img src=x>"));
        assert!(!html.contains("purchase_id="));
    }
}
