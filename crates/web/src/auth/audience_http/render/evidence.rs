//! Shared presentation of the same book and group evidence.
use super::time;
use crate::pages::escape_html;
use std::fmt::Write;
use tor_recommendations::services::audience::{
    books::{
        BookDiscoveryCounts, BookDiscoveryMetrics, BookDiscoveryObservation, BookDiscoveryPeriod,
    },
    DiscoveryHealth,
};

pub(super) fn encode(value: &str) -> String {
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
    prefix: &str,
    current: &BookDiscoveryMetrics,
    previous: &BookDiscoveryMetrics,
) {
    write!(body, r#"<details class="book-section-card audience-source-card" data-testid="{prefix}-comparison">
        <summary>Изменение к предыдущему периоду</summary><dl>"#).expect("String");
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

pub(super) fn period(
    body: &mut String,
    prefix: &str,
    key: &str,
    label: &str,
    period: &BookDiscoveryPeriod,
) {
    write!(body, r#"<section class="book-section-card audience-source-card" data-testid="{prefix}-{key}" data-period="{key}">
        <h3>{label}</h3><p>С {} до {} UTC.</p>"#, escape_html(&period.from_day), escape_html(&period.until_day_exclusive)).expect("String");
    if period.includes_partial_today {
        body.push_str("<p>Включает незавершённый сегодняшний день.</p>");
    }
    metrics(body, &period.totals);
    counts(body, &period.counts);
    body.push_str("</section>");
}

pub(super) fn counts(body: &mut String, counts: &BookDiscoveryCounts) {
    body.push_str("<h4>Состояние книг</h4><dl>");
    for (key, label, value) in [
        (
            "no-observations",
            "Нет сохранённых наблюдений",
            counts.no_observations,
        ),
        (
            "observed-without-starts",
            "Есть наблюдения, первых чтений нет",
            counts.observed_without_starts,
        ),
        ("with-starts", "Есть первые чтения", counts.with_starts),
    ] {
        write!(
            body,
            r#"<dt>{label}</dt><dd data-state="{key}">{value}</dd>"#
        )
        .expect("String");
    }
    body.push_str("</dl>");
}

pub(super) fn health(body: &mut String, prefix: &str, health: &DiscoveryHealth) {
    write!(body, r#"<section class="book-section-card" data-testid="{prefix}-health"><h3>Обновление данных</h3>"#).expect("String");
    if let Some(last_success) = &health.last_success_utc {
        write!(
            body,
            "<p>Последнее подтверждение обработки: {}.</p>",
            time(last_success)
        )
        .expect("String");
    } else {
        body.push_str("<p>Обработка ещё не подтверждена. Полнота свежих данных неизвестна.</p>");
    }
    if !health.fresh {
        body.push_str(
            "<p>Нет подтверждения за последние 30 секунд. Показанные итоги могут отставать.</p>",
        );
    }
    if health.rec_dirty || health.organic_dirty {
        body.push_str(
            "<p>Есть изменения, ожидающие пересчёта. Итоги могут измениться после обработки.</p>",
        );
    }
    body.push_str("</section>");
}
