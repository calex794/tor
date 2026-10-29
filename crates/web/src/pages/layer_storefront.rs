//! S031: NEwBOO-inspired public Layer offer and private purchased-term pages.
//! All names are escaped; no plaintext paid chapter, S3 token or buyer ID.
use super::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_commerce::repositories::layer_storefront::{LayerStorefront, LayerTermCard};

fn rubles(minor: i64) -> String {
    format!("{},{:02} ₽", minor / 100, minor % 100)
}

fn kind_label(kind: &str) -> &'static str {
    if kind == "lifetime" {
        "Бессрочный доступ с будущими пополнениями"
    } else {
        "30 дней без автоматического продления"
    }
}

pub fn offer_page(
    offer: &LayerStorefront,
    viewer: Option<i64>,
    csrf: Option<&str>,
    key: Option<&str>,
    purchased: bool,
    checkout_enabled: bool,
) -> Html<String> {
    let title = escape_html(&offer.title);
    let mut list = String::new();
    if offer.public_books.is_empty() {
        list.push_str(
            "<div class=\"ui-empty-state\"><h2 class=\"ui-empty-state__title\">В пакете пока нет опубликованных книг</h2><p class=\"ui-empty-state__text\">Покупка пока недоступна.</p></div>",
        );
    } else {
        list.push_str("<div class=\"layer-storefront-books\">");
        for book in &offer.public_books {
            let book_title = escape_html(&book.title);
            let cover = if book.has_cover {
                format!(
                    "<img src=\"/covers/{}/card.webp\" alt=\"\" loading=\"lazy\" decoding=\"async\">",
                    book.id
                )
            } else {
                format!(
                    "<span>{}</span>",
                    escape_html(&book.title.chars().next().unwrap_or('К').to_string())
                )
            };
            write!(
                list,
                "<a class=\"layer-storefront-book\" href=\"/books/{}\"><span class=\"layer-storefront-book__cover\" aria-hidden=\"true\">{cover}</span><span class=\"layer-storefront-book__title\">{book_title}</span></a>",
                book.id
            )
            .expect("write book card");
        }
        list.push_str("</div>");
        if offer.public_book_count > offer.public_books.len() as i64 {
            list.push_str("<p class=\"layer-note\">Показаны первые 40 опубликованных книг.</p>");
        }
    }
    let price = rubles(offer.price_minor);
    let kind = kind_label(&offer.term_kind);
    let mut action = String::new();
    if offer.public_book_count == 0 {
        action.push_str("<p class=\"layer-note\" role=\"status\">Оплата станет доступна после публикации книг.</p>");
    } else if viewer == Some(offer.owner_id) {
        action.push_str("<p class=\"layer-note\">Автор не покупает собственный пакет.</p>");
    } else if offer.has_access && offer.term_kind == "lifetime" {
        action.push_str("<p class=\"ui-alert ui-alert--success\" role=\"status\">Этот пакет уже доступен в вашем аккаунте.</p><a class=\"ui-button ui-button--primary\" href=\"/me/library/layers\">Мои пакеты</a>");
    } else if !checkout_enabled {
        action.push_str("<p class=\"layer-note\" role=\"status\">Покупка временно отключена. Уже оплаченные сроки сохраняются.</p>");
    } else if viewer.is_none() {
        action.push_str("<a class=\"layer-offer__box layer-offer__box--button\" href=\"/login\">Войти и приобрести доступ</a>");
    } else if let (Some(csrf), Some(key)) = (csrf, key) {
        let button = if offer.has_access {
            "Продлить ещё на 30 дней"
        } else {
            "Приобрести пакет"
        };
        write!(
            action,
            "<form method=\"post\" action=\"/layers/{}/purchase\" class=\"layer-purchase-form\">
             <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">
             <input type=\"hidden\" name=\"idempotency_key\" value=\"{}\">
             <input type=\"hidden\" name=\"expected_offer_id\" value=\"{}\">
             <input type=\"hidden\" name=\"expected_version_id\" value=\"{}\">
             <input type=\"hidden\" name=\"expected_price_minor\" value=\"{}\">
             <label class=\"layer-purchase-confirm\"><input type=\"checkbox\" name=\"confirm\" value=\"1\">
             <span>Подтверждаю разовую покупку за {price}. {kind}. Автосписаний нет.</span></label>
             <button type=\"submit\" class=\"layer-offer__box layer-offer__box--button\">{button}</button>
             </form>",
            offer.id,
            escape_html(csrf),
            escape_html(key),
            offer.offer_id,
            offer.version_id,
            offer.price_minor
        )
        .expect("write purchase form");
    } else {
        action.push_str("<p class=\"layer-note\" role=\"status\">Покупка сейчас отключена. Уже оплаченный доступ сохраняется.</p>");
    }
    let notice = if purchased && offer.has_access {
        "<p class=\"ui-alert ui-alert--success\" role=\"status\">Доступ к пакету сейчас действует. Его срок указан в личной библиотеке.</p>"
    } else {
        ""
    };
    let content = format!(
        "<section class=\"layers-page\"><div class=\"container\">
         <header class=\"ui-page-header\"><div><p class=\"ui-page-header__kicker\">Пакет доступа</p>
         <h1 class=\"ui-page-header__title\">{title}</h1>
         <p class=\"ui-page-header__text\">Книги автора в одном пакете. Условия доступа указаны до оплаты.</p></div></header>
         {notice}
         <div class=\"layers-public-layout\">
           <section class=\"layer-panel\" aria-labelledby=\"layer-books-title\">
             <h2 id=\"layer-books-title\">Состав пакета</h2>
             <p class=\"layer-note\">Опубликованных книг: {}. Состав может пополняться по правилам выбранного доступа.</p>
             {list}
           </section>
           <aside class=\"layer-panel layer-storefront-offer\" aria-label=\"Условия покупки\">
             <div class=\"layer-offer\"><div class=\"layer-offer__box\">
               <span class=\"layer-offer__label\">{kind}</span>
               <strong class=\"layer-offer__price\">{price}</strong>
               <span class=\"layer-offer__meta\">{} книг</span>
             </div></div>
             {action}
             <p class=\"layer-note\">Оплата только из внутреннего кошелька. Автоматического продления нет.</p>
           </aside>
         </div>
         <p class=\"layer-back\"><a href=\"/catalog\">В каталог</a> · <a href=\"/me/library/layers\">Мои пакеты</a></p>
         </div></section>",
        offer.public_book_count, offer.public_book_count
    );
    layout(
        &offer.title,
        "Состав пакета и условия разовой покупки.",
        &content,
    )
}

pub fn purchase_error(title: &str, message: &str, layer: i64) -> Html<String> {
    let content = format!(
        "<section class=\"layers-page\"><div class=\"container\"><div class=\"layer-panel layer-error\">
         <p class=\"ui-page-header__kicker\">Покупка пакета</p><h1>{}</h1><p>{}</p>
         <a class=\"ui-button ui-button--primary\" href=\"/layers/{layer}\">Вернуться к пакету</a>
         </div></div></section>",
        escape_html(title),
        escape_html(message),
    );
    layout(title, "Результат операции с пакетом.", &content)
}

fn state_label(state: &str) -> &'static str {
    match state {
        "active" => "Доступен",
        "pending" => "Оплачен · начнётся позже",
        "expired" => "Срок истёк",
        _ => "Доступ отозван",
    }
}

pub fn purchased_page(terms: &[LayerTermCard], paid_reader_available: bool) -> Html<String> {
    let mut cards = String::new();
    if terms.is_empty() {
        cards.push_str(
            "<div class=\"ui-empty-state\"><h2 class=\"ui-empty-state__title\">Купленных пакетов пока нет</h2><p class=\"ui-empty-state__text\">Перейдите в каталог, чтобы выбрать книги.</p><a class=\"ui-button ui-button--primary\" href=\"/catalog\">Открыть каталог</a></div>",
        );
    }
    for term in terms {
        let title = escape_html(&term.title);
        let kind = kind_label(&term.term_kind);
        let state = state_label(&term.state);
        let dates = if let Some(expires) = &term.expires_utc {
            format!(
                "С {} по {} (UTC)",
                escape_html(&term.starts_utc),
                escape_html(expires)
            )
        } else {
            format!("С {} · бессрочно", escape_html(&term.starts_utc))
        };
        let read_link = match (paid_reader_available, term.first_readable_chapter) {
            (true, Some(chapter_id)) if term.state == "active" => {
                format!(
                    "<a class=\"ui-button ui-button--primary\" href=\"/read/layer/{chapter_id}\">Читать главу</a>"
                )
            }
            _ => String::new(),
        };
        write!(
            cards,
            "<article class=\"layer-panel layer-term-card\">
             <span class=\"layer-term-card__status\">{state}</span>
             <h2>{title}</h2><p>{kind}</p>
             <p class=\"layer-note\">Покупка №{} · {dates}</p>
             <p class=\"layer-term-card__price\">{}</p>
             <a class=\"ui-button ui-button--ghost\" href=\"/layers/{}\">Открыть пакет</a>
             {read_link}
             </article>",
            term.term_id,
            rubles(term.price_minor),
            term.layer_id
        )
        .expect("write purchased Layer");
    }
    let body = format!(
        "<section class=\"layers-page\"><div class=\"container\">
         <nav class=\"dashboard-nav\" aria-label=\"Разделы библиотеки\">
         <a class=\"dashboard-nav__item\" href=\"/me/library\">Библиотека</a>
         <a class=\"dashboard-nav__item is-active\" href=\"/me/library/layers\" aria-current=\"page\">Мои пакеты</a></nav>
         <header class=\"ui-page-header\"><div><p class=\"ui-page-header__kicker\">Личная библиотека</p>
         <h1 class=\"ui-page-header__title\">Мои пакеты доступа</h1>
         <p class=\"ui-page-header__text\">Здесь видны купленные сроки, продления и отозванные права.</p></div></header>
         <div class=\"layer-term-grid\">{cards}</div></div></section>"
    );
    layout(
        "Мои пакеты доступа",
        "Покупки и оплаченные сроки пакетов.",
        &body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d05_offer_uses_real_newboo_components_and_escapes_untrusted_titles() {
        let offer = LayerStorefront {
            id: 8,
            title: "<script>alert('bad')</script>".into(),
            owner_id: 10,
            offer_id: 20,
            version_id: 21,
            price_minor: 1234,
            term_kind: "prepaid_30d".into(),
            public_book_count: 1,
            public_books: vec![
                tor_commerce::repositories::layer_storefront::StorefrontBook {
                    id: 33,
                    title: "Книга <img src=x>".into(),
                    has_cover: false,
                },
            ],
            has_access: false,
        };
        let page = offer_page(&offer, Some(40), Some("csrf"), Some("key"), false, true).0;
        assert!(page.contains("layers-public-layout"));
        assert!(page.contains("layer-storefront-books"));
        assert!(page.contains("href=\"/books/33\""));
        assert!(page.contains("12,34 ₽"));
        assert!(page.contains("Подтверждаю разовую покупку"));
        assert!(page.contains("30 дней без автоматического продления"));
        assert!(page.contains("&lt;script&gt;"));
        assert!(page.contains("&lt;img src=x&gt;"));
        assert!(!page.contains("<script>alert"));
        assert!(!page.contains("<img src=x>"));
        let guest = offer_page(&offer, None, None, None, false, true).0;
        assert!(guest.contains("Войти и приобрести доступ"));
        assert!(!guest.contains("csrf_token"));
        let disabled_guest = offer_page(&offer, None, None, None, false, false).0;
        assert!(disabled_guest.contains("Покупка временно отключена"));
        assert!(!disabled_guest.contains("Войти и приобрести доступ"));
    }

    #[test]
    fn d05_lifetime_and_empty_offer_cannot_render_purchase_form() {
        let mut offer = LayerStorefront {
            id: 1,
            title: "Тест".into(),
            owner_id: 10,
            offer_id: 2,
            version_id: 3,
            price_minor: 100,
            term_kind: "lifetime".into(),
            public_book_count: 1,
            public_books: Vec::new(),
            has_access: true,
        };
        let html = offer_page(&offer, Some(12), Some("csrf"), Some("key"), true, true).0;
        assert!(html.contains("Этот пакет уже доступен"));
        assert!(!html.contains("layer-purchase-form"));
        offer.has_access = false;
        offer.public_book_count = 0;
        let html = offer_page(&offer, Some(12), Some("csrf"), Some("key"), false, true).0;
        assert!(!html.contains("layer-purchase-form"));
    }
}
