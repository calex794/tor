//! H/S034: NEwBOO-inspired paid-reader shell. No paid body in HTML or DOM
//! before A/D/B/G grants and browser WebCrypto authentication.
use super::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_commerce::repositories::entitlements::{ReadablePurchase, ReadablePurchasesPage};

/// NEwBOO's hide_site_chrome mode: keep only the reader topbar.
/// Unlike the ordinary layout, this document cannot accidentally restore
/// global navigation, login links, or the site footer around a paid edition.
fn reader_layout(title: &str, description: &str, content: &str) -> Html<String> {
    Html(
        include_str!("../../templates/reader_layout.html")
            .replace("<!--PAGE_TITLE-->", &escape_html(title))
            .replace("<!--PAGE_DESCRIPTION-->", &escape_html(description))
            .replace("<!--PAGE_CONTENT-->", content),
    )
}

pub fn paid_reader_shell(
    edition: &ReadablePurchase,
    csrf: &str,
    private_s3_origin: &str,
) -> Html<String> {
    // D already authorized the pinned edition. Never render paid Markdown,
    // scoped S3 links, DEKs or a client-supplied title in this HTML shell.
    let book = escape_html(&edition.book_title);
    let chapter = escape_html(&edition.chapter_title);
    let chapter_label = format!("Глава {}. {chapter}", edition.chapter_no);
    let body = format!(
        r#"<section class="section reader-page reader-private reader-paid" data-paid-chapter-reader
             data-chapter-id="{chapter_id}" data-purchase-id="{purchase_id}" data-csrf-token="{csrf}"
             data-s3-origin="{s3_origin}" aria-labelledby="reader-title">
           <div class="container reader-width">
             <header class="reader-topbar" aria-label="Панель чтения">
               <a class="reader-topbar__back" href="/me/purchases"><span aria-hidden="true">←</span> К купленным главам</a>
               <div class="reader-topbar__title">
                 <span class="reader-topbar__book" title="{book}">{book}</span>
                 <span class="reader-topbar__chapter" title="{chapter_label}">{chapter_label}</span>
               </div>
             </header>
             <div class="reader-page__header">
               <p class="reader-page__meta">Купленная редакция {version_no}</p>
               <h1 id="reader-title" class="reader-page__title">{chapter_label}</h1>
               <p class="reader-page__subtitle" data-paid-status role="status">
                 Подготавливаем зашифрованную главу…
               </p>
             </div>
             <div class="reader-resume" data-paid-resume aria-live="polite"></div>
             <article class="chapter-content reader-markdown reader-paid__content"
                 data-paid-content aria-label="Текст купленной главы"></article>
             <p class="reader-page__return"><a class="text-link" href="/me/purchases">К купленным главам</a></p>
           </div>
           <script type="module" src="/assets/newboo-paid-reader.mjs"></script>
         </section>"#,
        chapter_id = edition.chapter_id,
        purchase_id = edition.purchase_id,
        csrf = escape_html(csrf),
        s3_origin = escape_html(private_s3_origin),
        book = book,
        chapter_label = chapter_label,
        version_no = edition.version_no,
    );
    reader_layout(
        &format!("{} — {}", edition.chapter_title, edition.book_title),
        "Чтение закреплённой купленной редакции после проверки доступа.",
        &body,
    )
}

/// A paid library entry exists only for a row already authorized by D.
/// Never interpolate chapter contents, S3 keys, DEKs or bearer links.
pub fn purchased_chapters_page(page: &ReadablePurchasesPage) -> Html<String> {
    let mut cards = String::new();
    if page.items.is_empty() {
        cards.push_str("<div class=\"library-empty\"><h2>Доступных покупок пока нет</h2><p>Здесь появятся купленные главы, на которые сохраняется право чтения.</p><a class=\"button\" href=\"/catalog\">К каталогу</a></div>");
    }
    for item in &page.items {
        let book = escape_html(&item.book_title);
        let chapter = escape_html(&item.chapter_title);
        write!(
            cards,
            "<article class=\"library-card paid-library__item\"><div><p class=\"eyebrow\">Купленная редакция {version_no}</p><h2>{book}</h2><p>Глава {chapter_no}. {chapter}</p></div><a class=\"button\" href=\"/read/paid/{chapter_id}?purchase_id={purchase_id}\">Читать купленную редакцию</a></article>",
            version_no = item.version_no,
            chapter_no = item.chapter_no,
            chapter_id = item.chapter_id,
            purchase_id = item.purchase_id,
        )
        .expect("write purchased library card");
    }
    let next = page.next_after.map_or_else(String::new, |cursor| {
        format!("<nav class=\"chapter-toc__more\" aria-label=\"Страницы покупок\"><a class=\"button button--small\" href=\"/me/purchases?after={cursor}\">Следующие покупки</a></nav>")
    });
    let body = format!(
        "<section class=\"section library-page paid-library\"><div class=\"container\"><nav class=\"breadcrumbs\" aria-label=\"Навигационная цепочка\"><a href=\"/me\">Мой кабинет</a><span aria-hidden=\"true\">/</span><a href=\"/me/library\">Библиотека</a><span aria-hidden=\"true\">/</span><span aria-current=\"page\">Купленные главы</span></nav><div class=\"library-heading\"><p class=\"eyebrow\">Моё чтение</p><h1>Купленные главы</h1><p>Здесь находятся доступные редакции, закреплённые за вашей покупкой. Новая редакция не заменяет ранее купленную.</p></div><div class=\"library-grid\">{cards}</div>{next}</div></section>"
    );
    layout(
        "Купленные главы",
        "Личный список доступных купленных редакций.",
        &body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paid_library_links_to_exact_purchased_edition_and_escapes_titles() {
        use tor_commerce::repositories::entitlements::ReadablePurchase;
        let page = ReadablePurchasesPage {
            items: vec![ReadablePurchase {
                purchase_id: 31,
                chapter_id: 23,
                version_id: 40,
                version_no: 1,
                chapter_no: 2,
                book_title: "Книга <script>опасно</script>".to_owned(),
                chapter_title: "Глава \"вторая\"".to_owned(),
            }],
            next_after: Some(31),
        };
        let html = purchased_chapters_page(&page).0;
        assert!(html.contains("href=\"/read/paid/23?purchase_id=31\""));
        assert!(html.contains("href=\"/me/purchases?after=31\""));
        assert!(html.contains("&lt;script&gt;опасно&lt;/script&gt;"));
        assert!(html.contains("&quot;вторая&quot;"));
        assert!(!html.contains("<script>опасно</script>"));
        assert!(!html.contains("dek_b64u"));
        assert!(!html.contains("ciphertext_url"));
    }

    #[test]
    fn h_paid_shell_contains_no_grant_or_plaintext_but_a_real_reader() {
        let edition = ReadablePurchase {
            purchase_id: 31,
            chapter_id: 23,
            version_id: 40,
            version_no: 1,
            chapter_no: 2,
            book_title: "Книга <script>опасно</script>".to_owned(),
            chapter_title: "Вторая \"глава\"".to_owned(),
        };
        let body = paid_reader_shell(
            &edition,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "https://private.storage.example",
        )
        .0;
        assert!(body.contains("reader-topbar"));
        assert!(body.contains("<body class=\"reader-standalone\">"));
        assert!(body.contains("<main id=\"main\" class=\"reader-standalone__main\">"));
        assert!(!body.contains("class=\"site-header\""));
        assert!(!body.contains("class=\"site-footer\""));
        assert!(!body.contains("class=\"site-logo\""));
        assert!(body.contains("<link rel=\"stylesheet\" href=\"/assets/site.css\">"));
        assert!(body.contains("<link rel=\"stylesheet\" href=\"/assets/newboo.css\">"));

        assert!(body.contains("data-paid-chapter-reader"));
        assert!(body.contains("newboo-paid-reader.mjs"));
        assert!(body.contains("data-purchase-id=\"31\""));
        assert!(body.contains("data-chapter-id=\"23\""));
        assert!(body.contains("Купленная редакция 1"));
        assert!(body.contains("Глава 2. Вторая &quot;глава&quot;"));
        assert!(body.contains("Книга &lt;script&gt;опасно&lt;/script&gt;"));
        assert!(!body.contains("<script>опасно</script>"));
        assert!(!body.contains("Оплаченная книга"));
        assert!(!body.contains("dek_b64u"));
        assert!(!body.contains("ciphertext_url"));
    }
}
