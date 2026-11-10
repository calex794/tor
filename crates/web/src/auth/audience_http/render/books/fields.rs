use crate::pages::escape_html;
use std::fmt::Write;
use tor_recommendations::services::audience::books::{
    BookDiscoveryMetrics, BookDiscoveryObservation, BookDiscoveryQuery, BookDiscoveryReport,
};

pub(super) fn filters(
    body: &mut String,
    query: &BookDiscoveryQuery,
    report: Option<&BookDiscoveryReport>,
) {
    body.push_str(r#"<form class="author-form audience-books-filters" id="audience-books-filters" method="get" action="/admin/audience/books">
        <div><label for="books-days">Длительность периода</label><select id="books-days" name="days">"#);
    for (value, label) in [
        (1, "1 день"),
        (7, "7 дней"),
        (30, "30 дней"),
        (90, "90 дней"),
    ] {
        let selected = if query.days == value { " selected" } else { "" };
        write!(
            body,
            r#"<option value="{value}"{selected}>{label}</option>"#
        )
        .expect("String");
    }
    body.push_str("</select></div>");
    for (key, label, value, options) in [
        (
            "genre",
            "Жанр",
            query.genre.as_deref(),
            report.map(|r| r.genres.as_slice()),
        ),
        (
            "tag",
            "Тег",
            query.tag.as_deref(),
            report.map(|r| r.tags.as_slice()),
        ),
    ] {
        write!(body, r#"<div><label for="books-{key}">{label}</label>
            <input id="books-{key}" name="{key}" list="books-{key}-options" maxlength="40" value="{}" placeholder="Любой" autocomplete="off">
            <datalist id="books-{key}-options">"#, escape_html(value.unwrap_or(""))).expect("String");
        for option in options.unwrap_or(&[]) {
            write!(body, r#"<option value="{}"></option>"#, escape_html(option)).expect("String");
        }
        body.push_str("</datalist></div>");
    }
    write!(body, r#"<p class="audience-books-filter-actions"><button class="button" type="submit">Показать</button>
        <a href="/admin/audience/books?days={}">Сбросить фильтры</a></p></form>"#, query.days).expect("String");
    if report.is_some_and(|r| r.filter_options_truncated) {
        body.push_str("<p>В подсказках показаны первые 100 значений. Нужный жанр или тег можно ввести целиком.</p>");
    }
}

pub(super) fn url(query: &BookDiscoveryQuery, after_id: Option<i64>) -> String {
    let mut url = format!("/admin/audience/books?days={}", query.days);
    for (key, value) in [("genre", &query.genre), ("tag", &query.tag)] {
        if let Some(value) = value {
            write!(url, "&{key}={}", encode(value)).expect("String");
        }
    }
    if let Some(id) = after_id {
        write!(url, "&after_id={id}").expect("String");
    }
    url
}

fn encode(value: &str) -> String {
    let mut result = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            result.push(char::from(byte));
        } else {
            write!(result, "%{byte:02X}").expect("String");
        }
    }
    result
}

fn values(metrics: &BookDiscoveryMetrics) -> [(&'static str, &'static str, Option<i64>); 5] {
    [
        (
            "issued",
            "Выдано ссылок из каталога и поиска",
            metrics.issued,
        ),
        (
            "impressions",
            "Наблюдаемые показы рекомендаций",
            metrics.impressions,
        ),
        ("clicks", "Учтённые переходы", metrics.clicks),
        ("book-opens", "Открытия книги", metrics.book_opens),
        ("starts", "Первые чтения книги", metrics.starts),
    ]
}

pub(super) fn metrics(body: &mut String, metrics: &BookDiscoveryMetrics) {
    body.push_str("<dl>");
    for (key, label, value) in values(metrics) {
        write!(
            body,
            r#"<dt>{label}</dt><dd data-metric="{key}">{}</dd>"#,
            value.map_or_else(|| "Нет данных".to_string(), |v| v.to_string())
        )
        .expect("String");
    }
    body.push_str("</dl>");
}

pub(super) fn comparison(
    body: &mut String,
    current: &BookDiscoveryMetrics,
    previous: &BookDiscoveryMetrics,
) {
    body.push_str(r#"<details class="book-section-card audience-source-card" data-testid="audience-books-comparison">
        <summary>Изменение к предыдущему периоду</summary><dl>"#);
    for ((key, label, current), (_, _, previous)) in
        values(current).into_iter().zip(values(previous))
    {
        let value = match (current, previous) {
            (Some(current), Some(previous)) => {
                format!("{:+}", i128::from(current) - i128::from(previous))
            }
            _ => "Нет сравнения".to_string(),
        };
        write!(
            body,
            r#"<dt>{label}</dt><dd data-change="{key}">{value}</dd>"#
        )
        .expect("String");
    }
    body.push_str("</dl><p>Разница только между сохранёнными измерениями. Неполные данные не заменяются нулями.</p></details>");
}

pub(super) fn observation(value: BookDiscoveryObservation) -> (&'static str, &'static str) {
    match value {
        BookDiscoveryObservation::NoObservations => {
            ("no-observations", "Нет сохранённых наблюдений")
        }
        BookDiscoveryObservation::ObservedWithoutStarts => (
            "observed-without-starts",
            "Есть наблюдения, первых чтений нет",
        ),
        BookDiscoveryObservation::WithStarts => ("with-starts", "Есть первые чтения"),
    }
}
