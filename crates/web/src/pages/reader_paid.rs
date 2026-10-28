//! H/S034: NEwBOO-inspired paid-reader shell. No paid body in HTML or DOM
//! before A/D/B/G grants and browser WebCrypto authentication.
use super::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_commerce::repositories::entitlements::{
    MixedBookToc, MixedTocAccess, MixedTocChapter, ReadablePurchase, ReadablePurchasesPage,
};

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

/// NEwBOO-like mixed table of contents: paid purchases are D-scoped,
/// free chapter links follow B's public rule, and locked chapters get no URL.
fn paid_book_toc_for(edition: &ReadablePurchase, toc: &MixedBookToc, bottom: bool) -> String {
    let mut entries = String::new();
    for chapter in &toc.chapters {
        let title = escape_html(&chapter.title);
        let label = format!(
            "<span class=\"reader-paid__toc-title\">Глава {}. {title}</span>",
            chapter.chapter_no
        );
        match chapter.access {
            MixedTocAccess::Purchased {
                purchase_id,
                version_no,
                ..
            } if chapter.chapter_id == edition.chapter_id && purchase_id == edition.purchase_id => {
                write!(
                    entries,
                    "<li class=\"reader-paid__toc-row reader-paid__toc-row--current\" data-toc-state=\"current\">
                       <span aria-current=\"page\">{label}<span class=\"reader-paid__toc-badge reader-paid__toc-badge--owned\">Читаю · редакция {version_no}</span></span>
                     </li>"
                )
                .expect("write current paid edition");
            }
            MixedTocAccess::Purchased {
                purchase_id,
                version_no,
                ..
            } => {
                write!(
                    entries,
                    "<li class=\"reader-paid__toc-row reader-paid__toc-row--owned\" data-toc-state=\"owned\">
                       <a data-paid-toc-link href=\"/read/paid/{}?purchase_id={purchase_id}\">{label}
                         <span class=\"reader-paid__toc-badge reader-paid__toc-badge--owned\">Куплено · редакция {version_no}</span>
                       </a>
                     </li>",
                    chapter.chapter_id
                )
                .expect("write paid edition link");
            }
            MixedTocAccess::PublicFree => {
                write!(
                    entries,
                    "<li class=\"reader-paid__toc-row reader-paid__toc-row--free\" data-toc-state=\"free\">
                       <a data-free-toc-link href=\"/reader/{}\">{label}
                         <span class=\"reader-paid__toc-badge reader-paid__toc-badge--free\">Бесплатно</span>
                       </a>
                     </li>",
                    chapter.chapter_id
                )
                .expect("write public free chapter link");
            }
            MixedTocAccess::Locked { price_minor } => {
                let price = price_minor.map_or_else(String::new, |minor| {
                    format!(" · {},{:02} ₽", minor / 100, minor % 100)
                });
                write!(
                    entries,
                    "<li class=\"reader-paid__toc-row reader-paid__toc-row--locked\" data-toc-state=\"locked\">
                       <span class=\"reader-paid__toc-locked\" aria-disabled=\"true\">{label}
                         <span class=\"reader-paid__toc-badge reader-paid__toc-badge--locked\">Не куплено{price}</span>
                       </span>
                     </li>"
                )
                .expect("write locked chapter metadata");
            }
        }
    }
    let overflow = if toc.truncated {
        "<p class=\"reader-paid__toc-more\">Показаны 100 ближайших глав. Остальные — в <a href=\"/me/purchases\">моих покупках</a> и каталоге.</p>"
    } else {
        ""
    };
    let position = if bottom {
        "reader-topbar__toc--bottom"
    } else {
        "reader-topbar__toc--top"
    };
    let toc_attr = if bottom {
        "data-paid-toc-bottom"
    } else {
        "data-paid-toc"
    };
    format!(
        "<details class=\"reader-topbar__toc {position}\" {toc_attr}>
          <summary aria-label=\"Оглавление книги\">Оглавление</summary>
          <div class=\"reader-paid__toc-menu\" role=\"group\" aria-label=\"Оглавление книги\">
            <div class=\"reader-paid__toc-head\">
              <strong>Главы книги</strong>
              <span>Бесплатные, купленные и закрытые</span>
            </div>
            <ol class=\"reader-paid__toc-list\">{entries}</ol>
            {overflow}
          </div>
         </details>"
    )
}

fn paid_book_toc(edition: &ReadablePurchase, toc: &MixedBookToc) -> String {
    paid_book_toc_for(edition, toc, false)
}

/// Render only a verified edition or a separately public free chapter.
/// Unpurchased paid neighbours are visible metadata, never navigable grants.
fn paid_nav_item(chapter: Option<&MixedTocChapter>, previous: bool) -> String {
    let (direction, accessible, arrow, rel) = if previous {
        ("Назад", "Предыдущая глава", "←", "prev")
    } else {
        ("Дальше", "Следующая глава", "→", "next")
    };
    let state = if previous { "prev" } else { "next" };
    let Some(chapter) = chapter else {
        return format!(
            "<span class=\"reader-paid__nav-card reader-paid__nav-card--disabled\" data-paid-nav-{state} aria-disabled=\"true\" aria-label=\"{accessible}\">
                <span class=\"reader-paid__nav-direction\">{arrow} {direction}</span>
                <span class=\"reader-paid__nav-name\">Нет доступной главы</span>
             </span>"
        );
    };
    let title = escape_html(&chapter.title);
    let label = format!("Глава {}. {title}", chapter.chapter_no);
    let (href, badge) = match chapter.access {
        MixedTocAccess::Purchased { purchase_id, .. } => (
            Some(format!(
                "/read/paid/{}?purchase_id={purchase_id}",
                chapter.chapter_id
            )),
            "Куплено",
        ),
        MixedTocAccess::PublicFree => {
            (Some(format!("/reader/{}", chapter.chapter_id)), "Бесплатно")
        }
        MixedTocAccess::Locked { .. } => (None, "Не куплено"),
    };
    match href {
        Some(href) => format!(
            "<a class=\"reader-paid__nav-card reader-paid__nav-card--active\" data-paid-nav-{state} rel=\"{rel}\" href=\"{href}\" aria-label=\"{accessible}\">
                <span class=\"reader-paid__nav-direction\">{arrow} {direction}</span>
                <span class=\"reader-paid__nav-name\">{label}</span>
                <span class=\"reader-paid__nav-badge\">{badge}</span>
             </a>"
        ),
        None => format!(
            "<span class=\"reader-paid__nav-card reader-paid__nav-card--locked\" data-paid-nav-{state} aria-disabled=\"true\" aria-label=\"{accessible} — глава не куплена\">
                <span class=\"reader-paid__nav-direction\">{arrow} {direction}</span>
                <span class=\"reader-paid__nav-name\">{label}</span>
                <span class=\"reader-paid__nav-badge\">{badge}</span>
             </span>"
        ),
    }
}

/// NEwBOO-style previous/next above and below the chapter. Do not jump over
/// a locked public chapter or fabricate purchases from progress/bookmarks.
fn paid_chapter_nav(edition: &ReadablePurchase, toc: &MixedBookToc, bottom: bool) -> String {
    let Some(index) = toc.chapters.iter().position(|item| {
        item.chapter_id == edition.chapter_id
            && matches!(
                item.access,
                MixedTocAccess::Purchased { purchase_id, version_id, .. }
                if purchase_id == edition.purchase_id && version_id == edition.version_id
            )
    }) else {
        return String::new();
    };
    let previous = index.checked_sub(1).and_then(|n| toc.chapters.get(n));
    let next = toc.chapters.get(index + 1);
    let region = if bottom { "внизу" } else { "вверху" };
    let position = if bottom { "bottom" } else { "top" };
    let bottom_toc = if bottom {
        paid_book_toc_for(edition, toc, true)
    } else {
        String::new()
    };
    format!(
        "<nav class=\"reader-paid__chapter-nav reader-paid__chapter-nav--{position}\"
              data-paid-chapter-nav aria-label=\"Переход между главами {region}\">
            {}
            {bottom_toc}
            {}
         </nav>",
        paid_nav_item(previous, true),
        paid_nav_item(next, false)
    )
}

pub fn paid_reader_shell(
    edition: &ReadablePurchase,
    toc: &MixedBookToc,
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
           <header class="reader-topbar" aria-label="Панель чтения">
               <a class="reader-topbar__back" href="/me/purchases" aria-label="К купленным главам"><span aria-hidden="true">←</span><span class="reader-topbar__back-label">К купленным главам</span></a>
               <div class="reader-topbar__title">
                 <span class="reader-topbar__book" title="{book}">{book}</span>
                 <span class="reader-topbar__chapter" title="{chapter_label}">{chapter_label}</span>
               </div>
               <div class="reader-topbar__actions">
                 <button class="reader-topbar__settings" type="button" data-paid-settings-open
                   aria-controls="paid-reader-settings" aria-expanded="false"
                   aria-haspopup="dialog" aria-label="Настройки чтения" title="Настройки чтения">Aa</button>
                 {toc_menu}
                 <button class="reader-topbar__theme" type="button" data-paid-theme-cycle
                   aria-label="Сменить тему" title="Сменить тему"><span aria-hidden="true">☾</span></button>
               </div>
             </header>
           <div class="container reader-width">
             <div id="paid-reader-settings" class="reader-paid__settings"
               data-paid-reader-settings hidden>
               <button type="button" class="reader-paid__settings-backdrop"
                 data-paid-settings-close aria-label="Закрыть настройки чтения"></button>
               <section class="reader-paid__settings-panel" role="dialog" aria-modal="true"
                 aria-labelledby="paid-reader-settings-title" tabindex="-1">
               <div class="reader-paid__settings-head">
                 <h2 id="paid-reader-settings-title">Настройки чтения</h2>
                 <button type="button" data-paid-settings-close aria-label="Закрыть настройки">Закрыть</button>
               </div>
               <div class="reader-paid__settings-fields">
                 <div class="reader-paid__settings-field" role="group" aria-label="Тема чтения">
                   <span>Тема</span>
                   <div class="reader-paid__themes">
                     <button type="button" data-paid-theme="light" aria-pressed="false">Светлая</button>
                     <button type="button" data-paid-theme="sepia" aria-pressed="false">Сепия</button>
                     <button type="button" data-paid-theme="dark" aria-pressed="false">Тёмная</button>
                   </div>
                 </div>
                 <div class="reader-paid__settings-field" role="group" aria-label="Шрифт чтения">
                   <span>Шрифт</span>
                   <div class="reader-paid__themes" data-paid-font-choice>
                     <button type="button" data-paid-font-family="serif" aria-pressed="true">С засечками</button>
                     <button type="button" data-paid-font-family="sans" aria-pressed="false">Без засечек</button>
                   </div>
                 </div>
                 <label class="reader-paid__settings-field">Размер текста
                   <input type="range" min="80" max="140" step="5" value="100" data-paid-font-scale>
                   <output data-paid-font-output>100%</output>
                 </label>
                 <label class="reader-paid__settings-field">Межстрочный интервал
                   <input type="range" min="150" max="220" step="5" value="185" data-paid-line-height>
                   <output data-paid-line-output>1.85</output>
                 </label>
                 <label class="reader-paid__settings-field">Ширина текста
                   <input type="range" min="620" max="980" step="20" value="860" data-paid-reader-width>
                   <output data-paid-width-output>860 px</output>
                 </label>
               </div>
               <button type="button" data-paid-settings-reset>Сбросить настройки</button>
               </section>
             </div>
             <div class="reader-page__header">
               <p class="reader-page__meta">Купленная редакция {version_no}</p>
               <h1 id="reader-title" class="reader-page__title">{chapter_label}</h1>
               <p class="reader-page__subtitle" data-paid-status role="status">
                 Подготавливаем зашифрованную главу…
               </p>
             </div>
             {top_nav}
             <div class="reader-resume" data-paid-resume aria-live="polite"></div>
             <article class="chapter-content reader-markdown reader-paid__content"
                 data-paid-content aria-label="Текст купленной главы"></article>
             <div class="reader-resume reader-paid__completion" data-paid-completion
                  aria-live="polite" hidden></div>
             {bottom_nav}
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
        toc_menu = paid_book_toc(edition, toc),
        top_nav = paid_chapter_nav(edition, toc, false),
        bottom_nav = paid_chapter_nav(edition, toc, true),
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
    fn mixed_book_toc_displays_nevboo_states_without_granting_closed_pages() {
        use tor_commerce::repositories::entitlements::MixedTocChapter;
        let edition = ReadablePurchase {
            purchase_id: 31,
            chapter_id: 23,
            version_id: 40,
            version_no: 1,
            chapter_no: 2,
            book_title: "Book".to_owned(),
            chapter_title: "Current".to_owned(),
        };
        let toc = MixedBookToc {
            chapters: vec![
                MixedTocChapter {
                    chapter_id: 21,
                    chapter_no: 1,
                    title: "Бесплатная <img src=x>".to_owned(),
                    access: MixedTocAccess::PublicFree,
                },
                MixedTocChapter {
                    chapter_id: 23,
                    chapter_no: 2,
                    title: "Current".to_owned(),
                    access: MixedTocAccess::Purchased {
                        purchase_id: 31,
                        version_id: 40,
                        version_no: 1,
                    },
                },
                MixedTocChapter {
                    chapter_id: 24,
                    chapter_no: 3,
                    title: "Купленная <script>x</script>".to_owned(),
                    access: MixedTocAccess::Purchased {
                        purchase_id: 32,
                        version_id: 42,
                        version_no: 1,
                    },
                },
                MixedTocChapter {
                    chapter_id: 25,
                    chapter_no: 4,
                    title: "Платная, без доступа".to_owned(),
                    access: MixedTocAccess::Locked {
                        price_minor: Some(175),
                    },
                },
            ],
            truncated: true,
        };
        let html = paid_book_toc(&edition, &toc);
        assert!(html.contains("aria-current=\"page\""));
        assert!(html.contains("href=\"/read/paid/24?purchase_id=32\""));
        assert!(html.contains("href=\"/reader/21\""));
        assert!(!html.contains("href=\"/read/paid/25"));
        assert!(!html.contains("href=\"/reader/25\""));
        assert!(!html.contains("href=\"/read/paid/23?purchase_id=31\""));
        assert!(html.contains("1,75 ₽"));
        assert!(html.contains("Бесплатно"));
        assert!(html.contains("Куплено"));
        assert!(html.contains("Не куплено"));
        assert!(html.contains("&lt;script&gt;x&lt;/script&gt;"));
        assert!(html.contains("&lt;img src=x&gt;"));
        assert!(!html.contains("<script>x</script>"));
        assert!(!html.contains("<img src=x>"));
        assert!(html.contains("100 ближайших"));
        assert!(!html.contains("ciphertext_url"));
        // Current bought chapter 2: prior free chapter 1 and next bought V1.
        let nav = paid_chapter_nav(&edition, &toc, false);
        assert!(nav.contains("rel=\"prev\" href=\"/reader/21\""));
        assert!(nav.contains("rel=\"next\" href=\"/read/paid/24?purchase_id=32\""));
        assert!(!nav.contains("href=\"/reader/25\""));
        // Chapter 3 is followed by unpurchased 4: show lock, no link.
        let next_edition = ReadablePurchase {
            purchase_id: 32,
            chapter_id: 24,
            version_id: 42,
            version_no: 1,
            chapter_no: 3,
            book_title: "Book".to_owned(),
            chapter_title: "Next".to_owned(),
        };
        let blocked = paid_chapter_nav(&next_edition, &toc, true);
        assert_eq!(blocked.matches("data-paid-toc-bottom").count(), 1);
        assert!(blocked.contains("reader-topbar__toc--bottom"));
        assert!(blocked.contains("href=\"/reader/21\""));
        // Active paid version is not a link back to itself.
        assert!(blocked.contains("href=\"/read/paid/23?purchase_id=31\""));
        assert!(!blocked.contains("href=\"/read/paid/24?purchase_id=32\""));
        assert!(blocked.contains("aria-current=\"page\""));
        assert!(!blocked.contains("href=\"/reader/25\""));
        assert!(blocked.contains("data-paid-nav-next aria-disabled=\"true\""));
        assert!(blocked.contains("Не куплено"));
        assert!(!blocked.contains("href=\"/read/paid/25"));
        assert!(!blocked.contains("href=\"/reader/25"));
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
        let toc = MixedBookToc {
            chapters: vec![tor_commerce::repositories::entitlements::MixedTocChapter {
                chapter_id: 23,
                chapter_no: 2,
                title: "Вторая \"глава\"".to_owned(),
                access: MixedTocAccess::Purchased {
                    purchase_id: 31,
                    version_id: 40,
                    version_no: 1,
                },
            }],
            truncated: false,
        };
        let body = paid_reader_shell(
            &edition,
            &toc,
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
        assert!(body.contains("data-paid-toc"));
        assert!(body.contains("Оглавление книги"));
        assert_eq!(body.matches("data-paid-chapter-nav").count(), 2);
        assert_eq!(body.matches("data-paid-toc-bottom").count(), 1);
        assert_eq!(body.matches("data-paid-toc>").count(), 1);
        assert!(body.contains("data-paid-nav-next aria-disabled=\"true\""));
        assert!(body.contains("data-paid-nav-prev aria-disabled=\"true\""));
        assert!(body.contains("aria-current=\"page\""));
        assert!(!body.contains("data-paid-toc-link"));
        assert!(body.contains("data-paid-settings-open"));
        assert!(body.contains("data-paid-font-family=\"serif\""));
        assert!(body.contains("data-paid-font-family=\"sans\""));
        assert!(body
            .contains("class=\"reader-paid__settings-panel\" role=\"dialog\" aria-modal=\"true\""));
        assert!(body.contains("data-paid-settings-close aria-label=\"Закрыть настройки чтения\""));
        assert!(body.contains("aria-haspopup=\"dialog\""));
        assert!(body.contains("data-paid-theme-cycle"));
        assert!(body.contains("aria-label=\"Сменить тему\""));
        let toolbar = body.find("<header class=\"reader-topbar\"").unwrap();
        let reader_column = body.find("<div class=\"container reader-width\"").unwrap();
        assert!(
            toolbar < reader_column,
            "NEwBOO toolbar must be full width, outside text column"
        );

        assert!(body.contains("data-paid-reader-settings"));
        assert!(body.contains("data-paid-font-scale"));
        assert!(body.contains("data-paid-reader-width"));
        assert!(body.contains("data-paid-theme=\"sepia\""));
        assert!(!body.contains("data-reader-toc-url"));
        assert!(body.contains("Глава 2. Вторая &quot;глава&quot;"));
        assert!(body.contains("Книга &lt;script&gt;опасно&lt;/script&gt;"));
        assert!(!body.contains("<script>опасно</script>"));
        assert!(!body.contains("Оплаченная книга"));
        assert!(!body.contains("dek_b64u"));
        assert!(!body.contains("ciphertext_url"));
    }
}
