use super::QueueSnapshot;
use crate::pages::{escape_html as e, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_contracts::exposure::ExposureSurface;
use tor_recommendations::domain::exposure::{
    Counts, ExposureFilter, ExposureReport, ObservationClass,
};

pub(super) fn page(
    q: &ExposureFilter,
    report: Option<&ExposureReport>,
    queue: QueueSnapshot,
) -> Html<String> {
    let mut body = String::from(
        r#"<section class="section"><div class="container audience-books-width"><p class="eyebrow">Администратору сайта</p><h1>Наблюдаемые показы книг</h1><p><a href="/admin/audience">Аудитория</a></p><p>Показ: не менее 60% карточки непрерывно видимо одну секунду при открытой активной вкладке. Браузер сообщает это по одноразовому подтверждению карточки. Это не уникальные люди и не оплачиваемые рекламные показы.</p>"#,
    );
    filters(&mut body, q);
    match report {
        None=>body.push_str(r#"<p role="alert" data-testid="exposure-unavailable">Данные временно недоступны. Нули не подставляются.</p>"#),
        Some(report)=>{
            write!(body,r#"<section class="book-section-card" data-testid="exposure-totals"><h2>Все подходящие книги: <span data-metric="books">{}</span></h2><p>Итоги до пагинации. Сейчас мало подтверждённых показов у {} книг; недостаточно наблюдений у {}. Класс в фильтре меняет список, а не общий итог.</p>"#,report.books,report.low_observed_books,report.unknown_books).expect("String");
            metrics(&mut body,report.current);body.push_str("<h3>Предыдущий период</h3>");metrics(&mut body,report.previous);
            write!(body,"<p>UTC; {days} календарных дней, включая текущий неполный день, и предыдущие {days} дней. Карточка относится к часу выдачи подтверждения; сигнал может прийти позднее. Снимок Unix UTC: {at}.</p></section>",days=q.days,at=report.checked_at_epoch).expect("String");
            if report.rows.is_empty(){body.push_str(r#"<p data-testid="exposure-empty">Нет книг для этой страницы и фильтров.</p>"#)}
            for book in &report.rows {
                write!(body,r#"<article class="book-section-card audience-book-row" data-testid="exposure-row" data-book-id="{}"><h2><a href="/books/{}">{}</a></h2><p data-testid="exposure-class">{}</p>"#,book.id,book.id,e(&book.title),class_label(book.counts.classification())).expect("String");
                metrics(&mut body,book.counts);
                write!(body,"<p>Предыдущий период: подтверждённых показов — {}. Сравнение описательное; причинный эффект продвижения не установлен.</p></article>",known(book.previous.known_views())).expect("String");
            }
            body.push_str(r#"<nav class="audience-books-pagination" aria-label="Страницы книг">"#);
            if q.after>0 {write!(body,r#"<a class="button button--quiet" href="{}">В начало</a>"#,e(&url(q,0))).expect("String");}
            if let Some(after)=report.next {write!(body,r#"<a class="button" data-testid="exposure-next" href="{}">Следующие 50</a>"#,e(&url(q,after))).expect("String");}
            body.push_str("</nav><section class=\"book-section-card\"><h2>Жанры и теги</h2><p>До 100 групп в алфавитном порядке; любую точную метку можно выбрать в форме. Каждая книга учитывается один раз внутри группы. Группы пересекаются: складывать их итоги нельзя.</p>");
            for group in &report.groups {
                let mut selected=q.clone();selected.group=Some((group.kind.clone(),group.label.clone()));selected.after=0;
                write!(body,r#"<p><a href="{}">{}: {}</a> — книг {}, зарегистрированных карточек {}, подтверждённых показов {}.</p>"#,e(&url(&selected,0)),if group.kind=="genre"{"Жанр"}else{"Тег"},e(if group.label.is_empty(){"Без метки"}else{&group.label}),group.books,group.counts.opportunities,known(group.counts.known_views())).expect("String");
            }
            body.push_str("</section>");
        }
    }
    body.push_str("<section class=\"book-section-card\"><h2>Границы наблюдения</h2><p>Зарегистрированные карточки — подтверждения, сохранённые сервером. Это не все отданные HTML-карточки. «Браузер увидел карточку в DOM» означает работоспособность измерения, не видимость в окне. Без JavaScript, при блокировке сети и потере очереди видимость неизвестна. Ноль подтверждённых показов не доказывает, что книгу никто не видел.</p><p>Класс «мало подтверждённых показов» доступен только при 20 и более подтверждениях DOM и их доле не менее 80% зарегистрированных карточек; затем порог — менее пяти показов. Это операционный порог для проверки размещения, не оценка спроса или качества книги. Клиентский сигнал может быть подделан и непригоден для расчётов с рекламодателем.</p><p>Сейчас доступные публичные книги с текущими жанрами и тегами. После скрытия книги её строки и суммы исключаются. Имена читателей, аккаунты, IP, сессии, поисковые запросы, переходы между страницами и история чтения не сохраняются. Подтверждение живёт 15 минут; повтор каждого вида события учитывается один раз. Клик не заменяет показ. Чтения из независимых отчётов не образуют здесь конверсию.</p>");
    write!(body,"<h3>Очередь текущего процесса</h3><p>Поставлено {}, сохранено {}, отброшено при заполнении {}, не сохранено или ошибка очистки {}. Эти диагностические счётчики сбрасываются при перезапуске и не доказывают полноту покрытия.</p></section></div></section>",queue.queued,queue.persisted,queue.dropped,queue.failed).expect("String");
    layout(
        "Наблюдаемые показы книг",
        "Показы карточек и границы наблюдения",
        &body,
    )
}
fn class_label(c: ObservationClass) -> &'static str {
    match c {
        ObservationClass::Unknown => "Недостаточно наблюдений",
        ObservationClass::LowObserved => "Мало подтверждённых показов",
        ObservationClass::Observed => "Порог наблюдаемых показов достигнут",
    }
}
fn known(value: Option<i64>) -> String {
    value.map_or_else(|| "неизвестно".into(), |n| n.to_string())
}
fn metrics(out: &mut String, c: Counts) {
    out.push_str("<dl>");
    for (key, label, value) in [
        (
            "opportunities",
            "Зарегистрированные карточки",
            c.opportunities.to_string(),
        ),
        ("rendered", "Подтверждения DOM", c.rendered.to_string()),
        ("views", "Подтверждённые показы", known(c.known_views())),
        (
            "clicks",
            "Подтверждённые клики",
            if c.rendered > 0 {
                c.clicks.to_string()
            } else {
                "неизвестно".into()
            },
        ),
        (
            "expired",
            "Истёкшие подтверждения без показа",
            c.expired_unobserved.to_string(),
        ),
        (
            "replayed",
            "Повторные сигналы, не увеличившие итог",
            c.replayed.to_string(),
        ),
    ] {
        write!(
            out,
            r#"<dt>{label}</dt><dd data-metric="{key}">{value}</dd>"#
        )
        .expect("String");
    }
    out.push_str("</dl>");
    let coverage = if c.opportunities > 0 {
        format!("{:.1}%", c.rendered as f64 * 100.0 / c.opportunities as f64)
    } else {
        "неизвестно".into()
    };
    write!(out,"<p>Подтверждения DOM / зарегистрированные карточки: {coverage}. Это покрытие сохранённых подтверждений, не всего трафика.</p>").expect("String");
}
fn select(out: &mut String, key: &str, label: &str, chosen: &str, values: &[(&str, &str)]) {
    write!(out,r#"<div><label for="exposure-{key}">{label}</label><select id="exposure-{key}" name="{key}">"#).expect("String");
    for (value, label) in values {
        write!(
            out,
            r#"<option value="{value}"{}>{label}</option>"#,
            if *value == chosen { " selected" } else { "" }
        )
        .expect("String");
    }
    out.push_str("</select></div>");
}
fn filters(out: &mut String, q: &ExposureFilter) {
    out.push_str(r#"<form class="author-form audience-books-filters" method="get" action="/admin/audience/exposure">"#);
    select(
        out,
        "days",
        "Дней в периоде",
        &q.days.to_string(),
        &[("1", "1"), ("7", "7"), ("30", "30"), ("90", "90")],
    );
    select(
        out,
        "surface",
        "Место",
        q.surface.map_or("", ExposureSurface::as_str),
        &[
            ("", "Все"),
            ("home", "Главная"),
            ("catalog", "Каталог"),
            ("search", "Поиск"),
            ("book_detail", "Страница книги"),
            ("reader_end", "После главы"),
        ],
    );
    select(
        out,
        "source",
        "Источник",
        q.editorial
            .map_or("", |e| if e { "editorial" } else { "organic" }),
        &[
            ("", "Все"),
            ("organic", "Каталог и рекомендации"),
            ("editorial", "Редакционное продвижение"),
        ],
    );
    select(
        out,
        "class",
        "Наблюдения",
        q.class.map_or("", ObservationClass::as_str),
        &[
            ("", "Все"),
            ("unknown", "Недостаточно наблюдений"),
            ("low", "Мало подтверждённых показов"),
            ("observed", "Порог достигнут"),
        ],
    );
    select(
        out,
        "group",
        "Точная метка",
        q.group.as_ref().map_or("", |g| g.0.as_str()),
        &[("", "Все"), ("genre", "Жанр"), ("tag", "Тег")],
    );
    write!(out,r#"<div><label for="exposure-label">Значение метки (пустое — без метки)</label><input id="exposure-label" name="label" maxlength="160" value="{}"></div><div><label for="exposure-campaign">Номер редакционной кампании</label><input id="exposure-campaign" name="campaign" type="number" min="1" value="{}"></div><p><button class="button" type="submit">Показать</button> <a href="/admin/audience/exposure">Сбросить</a></p></form>"#,e(q.group.as_ref().map_or("",|g|g.1.as_str())),q.campaign.map(|v|v.to_string()).unwrap_or_default()).expect("String");
}
fn encode(value: &str) -> String {
    let mut out = String::new();
    for b in value.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(char::from(b))
        } else {
            write!(out, "%{b:02X}").expect("String")
        }
    }
    out
}
fn url(q: &ExposureFilter, after: i64) -> String {
    let mut url = format!("/admin/audience/exposure?days={}", q.days);
    if let Some(surface) = q.surface {
        write!(url, "&surface={}", surface.as_str()).expect("String");
    }
    if let Some(editorial) = q.editorial {
        write!(
            url,
            "&source={}",
            if editorial { "editorial" } else { "organic" }
        )
        .expect("String");
    }
    if let Some(campaign) = q.campaign {
        write!(url, "&campaign={campaign}").expect("String");
    }
    if let Some(class) = q.class {
        write!(url, "&class={}", class.as_str()).expect("String");
    }
    if let Some((kind, label)) = &q.group {
        write!(url, "&group={kind}&label={}", encode(label)).expect("String");
    }
    if after > 0 {
        write!(url, "&after={after}").expect("String");
    }
    url
}
