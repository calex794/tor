use super::console::PageQuery;
use crate::pages::{escape_html as esc, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_accounts::services::security::generate_session_token;
use tor_ads::domain::paid::*;

pub(super) fn money(value: i64) -> String {
    format!("{}.{:02} ₽", value / 100, value % 100)
}
fn hidden(csrf: &str) -> String {
    format!(
        r#"<input type="hidden" name="csrf_token" value="{}"><input type="hidden" name="command_key" value="{}">"#,
        esc(csrf),
        generate_session_token()
    )
}
pub(super) struct ConsoleView<'a> {
    pub q: &'a PageQuery,
    pub csrf: &'a str,
    pub flags: AdFlags,
    pub offers: &'a [DailyOffer],
    pub packages: &'a [ChapterPackage],
    pub books: &'a [OwnedBook],
    pub daily: &'a [Campaign],
    pub chapter: &'a [Campaign],
    pub publisher: &'a (PublisherBalance, Vec<PublisherCredit>),
}
pub(super) fn console(view: ConsoleView<'_>) -> Html<String> {
    let ConsoleView {
        q,
        csrf,
        flags,
        offers,
        packages,
        books,
        daily,
        chapter,
        publisher,
    } = view;
    let mut body = String::from(
        r#"<div class="ads-console"><a href="/me">Личный кабинет</a><h1>Реклама книг</h1><p>Два способа продвижения: размещение по дням и оплачиваемые показы под бесплатными главами.</p>"#,
    );
    if let Some(message) = q.changed.as_deref().and_then(|v| match v {
        "purchased" => Some("Кампания оплачена. Заказ сохранён в списке ниже."),
        "cancelled" => Some("Кампания остановлена. Неиспользованный остаток возвращён в кошелёк."),
        "cancelling" => {
            Some("Размещение остановлено. Завершаем расчёт подтверждённых показов перед возвратом.")
        }
        "settings" => Some("Настройки издателя сохранены."),
        "credited" => Some("Доступные начисления переведены в кошелёк."),
        _ => None,
    }) {
        write!(body, "<p role=\"status\">{message}</p>").unwrap();
    }
    let book_options = books
        .iter()
        .map(|b| {
            format!(
                "<option value=\"{}\"{}>{}</option>",
                b.id,
                if q.book == Some(b.id) {
                    " selected"
                } else {
                    ""
                },
                esc(&b.title)
            )
        })
        .collect::<String>();
    body.push_str("<section><h2>Размещение по дням</h2><p>Весь срок оплачивается заранее. Даты считаются по UTC. При отмене возвращается стоимость будущих полных дней; начавшийся день оплачивается целиком.</p>");
    if flags.daily_purchase && !offers.is_empty() && !books.is_empty() {
        write!(body,r#"<form method="post" action="/me/ads/daily" data-ads-daily>{}<label>Книга<select name="book_id" required>{book_options}</select></label><label>Размещение и тариф<select name="offer" required>"#,hidden(csrf)).unwrap();
        for o in offers {
            write!(body,"<option value=\"{}:{}:{}\" data-rate-minor=\"{}\">{} — {} в день; {}–{} дней; условия {}</option>",o.id,o.revision,esc(&o.slot),o.rate_minor,esc(slot_label(&o.slot)),money(o.rate_minor),o.min_days,o.max_days,esc(&o.terms_version)).unwrap();
        }
        body.push_str(r#"</select></label><label>Первый день (UTC)<input name="start_date" type="date" required></label><label>Количество дней<input name="days" type="number" min="1" max="366" value="1" required></label><output data-ads-total aria-live="polite"></output><label><input type="checkbox" name="accept_terms" value="true" required> Подтверждаю полную предоплату из кошелька и правила отмены</label><button type="submit">Оплатить размещение</button></form>"#);
    } else {
        body.push_str("<p>Покупка размещений сейчас недоступна.</p>");
    }
    campaigns(&mut body, daily, csrf, "daily");
    body.push_str("</section><section><h2>Показы под главами</h2><p>Бюджет резервируется в кошельке. Расход учитывается после подтверждённой видимости карточки: не менее 60% в течение секунды. При отмене возвращается неиспользованная часть пакета.</p>");
    if flags.chapter_purchase && !packages.is_empty() && !books.is_empty() {
        write!(body,r#"<form method="post" action="/me/ads/chapter">{}<label>Книга<select name="book_id" required>{book_options}</select></label><label>Пакет<select name="package" required>"#,hidden(csrf)).unwrap();
        for p in packages {
            write!(body,"<option value=\"{}:{}\">{} показов — {}; доля издателя {}.{:02}%; удержание {} с; условия {}</option>",p.id,p.revision,p.target_impressions,money(p.price_minor),p.publisher_share_bps/100,p.publisher_share_bps%100,p.hold_seconds,esc(&p.terms_version)).unwrap();
        }
        body.push_str(r#"</select></label><label><input type="checkbox" name="accept_terms" value="true" required> Подтверждаю резервирование полной стоимости пакета из кошелька</label><button type="submit">Оплатить пакет показов</button></form>"#);
    } else {
        body.push_str("<p>Покупка пакетов сейчас недоступна.</p>");
    }
    campaigns(&mut body, chapter, csrf, "chapter");
    body.push_str("</section>");
    let (b, credits) = publisher;
    write!(body,r#"<section id="publisher"><h2>Доход от рекламы в моих книгах</h2><p>Рекламные карточки могут появляться только под открытыми бесплатными главами. Включение разрешает новые размещения; отключение сохраняет уже заработанные начисления.</p><form method="post" action="/me/ads/publisher/settings"><input type="hidden" name="csrf_token" value="{}"><input type="hidden" name="expected_revision" value="{}"><label>Размещения<select name="enabled"><option value="false"{}>Выключены</option><option value="true"{}>Включены</option></select></label><button>Сохранить</button></form><dl><dt>Ожидают окончания удержания</dt><dd>{}</dd><dt>Доступны</dt><dd>{}</dd><dt>Переведено в кошелёк</dt><dd>{}</dd></dl><form method="post" action="/me/ads/publisher/credit">{}<button>Перевести доступные начисления в кошелёк</button></form>"#,
  esc(csrf),b.revision,if !b.enabled{" selected"}else{""},if b.enabled{" selected"}else{""},money(b.pending_minor),money(b.available_minor),money(b.credited_minor),hidden(csrf)).unwrap();
    body.push_str("<div class=\"ads-table\"><table><caption>Начисления издателя</caption><thead><tr><th>Сумма</th><th>Состояние</th><th>Окончание удержания</th></tr></thead><tbody>");
    for c in credits {
        write!(
            body,
            "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
            money(c.amount_minor),
            esc(state_label(&c.status)),
            esc(&c.clear_at)
        )
        .unwrap();
    }
    body.push_str("</tbody></table></div>");
    if credits.len() == 100 {
        write!(
            body,
            "<a href=\"/me/ads?publisher_before={}#publisher\">Следующие начисления</a>",
            credits.last().unwrap().id
        )
        .unwrap();
    }
    body.push_str("</section></div>");
    let html = layout("Реклама книг", "Рекламные кампании и доход издателя", &body);
    Html(
        html.0
            .replace(
                "</head>",
                "<link rel=\"stylesheet\" href=\"/assets/paid-ads.css\"></head>",
            )
            .replace(
                "</body>",
                "<script src=\"/assets/paid-ads.js\" defer></script></body>",
            ),
    )
}
fn campaigns(body: &mut String, rows: &[Campaign], csrf: &str, p: &str) {
    body.push_str("<div class=\"ads-table\"><table><caption>Мои кампании</caption><thead><tr><th>Книга</th><th>Состояние</th><th>Бюджет</th><th>Расход</th><th>Возврат</th><th>Результат</th><th>Действие</th></tr></thead><tbody>");
    for c in rows {
        write!(body,"<tr data-ads-campaign=\"{}\"><td><a href=\"/books/{}\">{}</a></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}/{} {}; клики: {}; открытия: {}; чтение: {}; библиотека: {}; покупки: {}</td><td>",
    c.id,c.book_id,esc(&c.title),esc(state_label(&c.status)),money(c.budget_minor),money(c.settled_minor),money(c.refunded_minor),c.reserved,c.target,if p=="daily"{"дней"}else{"показов"},c.clicks,c.opens,c.reads,c.library_adds,c.purchases).unwrap();
        if matches!(c.status.as_str(), "funded" | "paused" | "completed") {
            write!(body,r#"<form method="post" action="/me/ads/{p}/{}/cancel">{}<input type="hidden" name="expected_revision" value="{}"><label>Причина отмены<input name="reason" maxlength="500" required></label><button>Остановить и вернуть остаток</button></form>"#,c.id,hidden(csrf),c.revision).unwrap();
        }
        body.push_str("</td></tr>");
    }
    body.push_str("</tbody></table></div>");
    if rows.len() == 100 {
        write!(
            body,
            "<a href=\"/me/ads?{p}_before={}\">Следующие кампании</a>",
            rows.last().unwrap().id
        )
        .unwrap();
    }
}
pub(super) fn slot_label(v: &str) -> &str {
    match v {
        "home_featured" => "Главная",
        "book_detail_related" => "Карточка книги",
        "chapter_end_free" => "После бесплатной главы",
        _ => v,
    }
}
fn state_label(v: &str) -> &str {
    match v {
        "funded" => "Оплачена",
        "paused" => "Приостановлена",
        "cancelling" => "Расчёт возврата",
        "cancelled" => "Отменена",
        "completed" => "Завершена",
        "pending" => "Удержание",
        "available" => "Доступно",
        "credited" => "В кошельке",
        _ => v,
    }
}
