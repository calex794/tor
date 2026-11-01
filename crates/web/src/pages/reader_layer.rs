//! D-05/H: protected Layer CURRENT chapter shell without direct-purchase claims.
//! This HTML never includes plaintext, object keys, S3 bearer URLs or DEKs.
use super::escape_html;
use axum::response::Html;

pub fn layer_reader_shell(chapter_id: i64, csrf: &str, s3_origin: &str) -> Html<String> {
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
                 <span class="reader-topbar__book">Подписка Layer</span>
                 <span class="reader-topbar__chapter">Глава {chapter_id}</span>
               </div>
             </header>
             <div class="container reader-width">
               <div class="reader-page__header">
                 <p class="reader-page__meta">Текущая опубликованная редакция Layer</p>
                 <h1 class="reader-page__title" id="reader-title">Глава {chapter_id}</h1>
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
        include_str!("../../templates/reader_layout.html")
            .replace("<!--PAGE_TITLE-->", &format!("Глава {chapter_id} — Layer"))
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
    fn layer_shell_contains_no_direct_purchase_or_s3_credentials() {
        let html = layer_reader_shell(42, "csrf_test", "https://private.example.test").0;
        assert!(html.contains("data-layer-chapter-reader"));
        assert!(html.contains("data-chapter-id=\"42\""));
        assert!(html.contains("newboo-layer-reader.mjs"));
        assert!(html.contains("https://private.example.test"));
        assert!(!html.contains("data-purchase-id"));
        assert!(!html.contains("ciphertext_url"));
        assert!(!html.contains("dek_b64u"));
        assert!(!html.contains("/api/v1/me/progress/purchased"));
    }
}
