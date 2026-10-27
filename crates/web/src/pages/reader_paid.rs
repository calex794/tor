//! H/S034: NEwBOO-inspired paid-reader shell. No paid body in HTML or DOM
//! before A/D/B/G grants and browser WebCrypto authentication.
use super::{escape_html, layout};
use axum::response::Html;

pub fn paid_reader_shell(
    chapter_id: &str,
    purchase_id: &str,
    csrf: &str,
    private_s3_origin: &str,
) -> Html<String> {
    let body = format!(
        r#"<section class="section reader-page reader-private reader-paid" data-paid-chapter-reader
             data-chapter-id="{}" data-purchase-id="{}" data-csrf-token="{}"
             data-s3-origin="{}" aria-labelledby="reader-title">
           <div class="container reader-width">
             <header class="reader-topbar" aria-label="Панель чтения">
               <a class="reader-topbar__back" href="/me/library"><span aria-hidden="true">←</span> Моя библиотека</a>
               <div class="reader-topbar__title">
                 <span class="reader-topbar__book">Оплаченная книга</span>
                 <span class="reader-topbar__chapter">Глава {}</span>
               </div>
             </header>
             <div class="reader-page__header">
               <p class="reader-page__meta">Приватная купленная редакция</p>
               <h1 id="reader-title" class="reader-page__title">Глава {}</h1>
               <p class="reader-page__subtitle" data-paid-status role="status">
                 Подготавливаем зашифрованную главу…
               </p>
             </div>
             <div class="reader-resume" data-paid-resume aria-live="polite"></div>
             <article class="chapter-content reader-markdown reader-paid__content"
                 data-paid-content aria-label="Текст купленной главы"></article>
             <p class="reader-page__return"><a class="text-link" href="/me/library">К моей библиотеке</a></p>
           </div>
           <script type="module" src="/assets/newboo-paid-reader.mjs"></script>
         </section>"#,
        escape_html(chapter_id),
        escape_html(purchase_id),
        escape_html(csrf),
        escape_html(private_s3_origin),
        escape_html(chapter_id),
        escape_html(chapter_id),
    );
    layout(
        "Купленная глава",
        "Чтение оплаченной редакции после проверки доступа.",
        &body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h_paid_shell_contains_no_grant_or_plaintext_but_a_real_reader() {
        let body = paid_reader_shell(
            "23",
            "31",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "https://private.storage.example",
        )
        .0;
        assert!(body.contains("reader-topbar"));
        assert!(body.contains("data-paid-chapter-reader"));
        assert!(body.contains("newboo-paid-reader.mjs"));
        assert!(body.contains("data-purchase-id=\"31\""));
        assert!(!body.contains("dek_b64u"));
        assert!(!body.contains("ciphertext_url"));
    }
}
