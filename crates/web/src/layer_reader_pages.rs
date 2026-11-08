//! D-05/H: protected Layer CURRENT chapter shell and scoped TOC.
//! Only metadata authorized by the Layer repository reaches this HTML.
use crate::pages::escape_html;
use axum::response::Html;
use tor_commerce::services::reader_views::LayerTocAnchor;

pub fn layer_reader_shell(anchor: &LayerTocAnchor, csrf: &str, s3_origin: &str) -> Html<String> {
    let chapter_id = anchor.chapter_id;
    let chapter_no = anchor.chapter_no;
    let title = escape_html(&anchor.chapter_title);
    let book = escape_html(&anchor.book_title);
    let chapter_label = format!("Глава {chapter_no}. {title}");
    let toc = format!(
        r#"<details class="reader-topbar__toc" data-layer-toc>
              <summary aria-label="Открыть оглавление">Оглавление</summary>
              <button class="reader-paid__toc-backdrop" type="button"
                data-layer-toc-backdrop tabindex="-1"
                aria-label="Закрыть оглавление"></button>
              <div class="reader-paid__toc-menu" data-layer-toc-modal
                role="dialog" aria-modal="true" aria-label="Оглавление Layer">
                <div class="reader-paid__toc-head">
                  <div><strong>Главы книги</strong>
                    <span>Только опубликованные главы с действующим доступом</span></div>
                  <button class="reader-paid__toc-close" data-layer-toc-close type="button"
                    aria-label="Закрыть оглавление">×</button>
                </div>
                <div class="reader-paid__toc-scroll" data-layer-toc-scroll
                  tabindex="0" aria-label="Прокрутка оглавления">
                  <ol class="reader-paid__toc-list" data-layer-toc-list>
                    <li class="reader-paid__toc-row reader-paid__toc-row--current"
                        data-toc-state="current" data-toc-id="{chapter_id}"
                        data-toc-no="{chapter_no}">
                      <span aria-current="page">
                        <span class="reader-paid__toc-title">{chapter_label}</span>
                        <span class="reader-paid__toc-badge reader-paid__toc-badge--owned">Layer</span>
                      </span>
                    </li>
                  </ol>
                </div>
                <p class="reader-paid__toc-more" data-layer-toc-status role="status">
                  Загрузить соседние главы
                </p>
              </div>
            </details>"#
    );
    let body = format!(
        r#"<section class="section reader-page reader-private reader-paid"
               data-layer-chapter-reader data-chapter-id="{chapter_id}"
               data-csrf-token="{csrf}" data-s3-origin="{s3_origin}"
               aria-labelledby="reader-title">
             <header class="reader-topbar" aria-label="Панель чтения Layer">
               <a class="reader-topbar__back" href="/me/library" aria-label="К библиотеке">
                 <span aria-hidden="true">←</span>
                 <span class="reader-topbar__back-label">К библиотеке</span>
               </a>
               <div class="reader-topbar__title">
                 <span class="reader-topbar__book" title="{book}">{book}</span>
                 <span class="reader-topbar__chapter" title="{chapter_label}">{chapter_label}</span>
               </div>
               <div class="reader-topbar__actions">{toc}</div>
             </header>
             <div class="container reader-width">
               <div class="reader-page__header">
                 <p class="reader-page__meta">Текущая опубликованная редакция Layer</p>
                 <h1 class="reader-page__title" id="reader-title">{chapter_label}</h1>
                 <p class="reader-page__subtitle" data-layer-status role="status">
                   Проверяем подписку…
                 </p>
               </div>
               <article class="chapter-content reader-markdown reader-paid__content"
                        data-layer-content aria-label="Текст главы Layer"></article>
               <p class="reader-page__return">
                 <a class="text-link" href="/me/library">К библиотеке</a>
               </p>
             </div>
             <script type="module" src="/assets/newboo-layer-reader.mjs"></script>
           </section>"#,
        csrf = escape_html(csrf),
        s3_origin = escape_html(s3_origin),
    );
    Html(
        include_str!("../templates/reader_layout.html")
            .replace("<!--PAGE_TITLE-->", &format!("{chapter_label} — {book}"))
            .replace(
                "<!--PAGE_DESCRIPTION-->",
                "Защищённое чтение текущей редакции по действующей подписке Layer.",
            )
            .replace("<!--PAGE_CONTENT-->", &body),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_shell_contains_safe_real_chapter_metadata_and_no_purchase_secrets() {
        let anchor = LayerTocAnchor {
            chapter_id: 42,
            chapter_no: 28,
            chapter_title: "Заголовок <script>alert(1)</script>".into(),
            book_title: "Книга <img src=x>".into(),
        };
        let html = layer_reader_shell(&anchor, "csrf_test", "https://private.example.test").0;
        assert!(html.contains("data-layer-chapter-reader"));
        assert!(html.contains("data-chapter-id=\"42\""));
        assert!(html.contains("Глава 28."));
        assert!(html.contains("data-layer-toc-modal"));
        assert!(html.contains("data-layer-toc-close"));
        assert!(html.contains("data-toc-no=\"28\""));
        assert!(html.contains("newboo-layer-reader.mjs"));
        assert!(html.contains("https://private.example.test"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(!html.contains("data-purchase-id"));
        assert!(!html.contains("ciphertext_url"));
        assert!(!html.contains("dek_b64u"));
        assert!(!html.contains("/api/v1/me/progress/purchased"));
    }
}
