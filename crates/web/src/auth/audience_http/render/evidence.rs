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
    body.push_str("<dl data-measurement=\"legacy-consented\">");
    for (key, label, value) in values(metrics) {
        write!(
            body,
            r#"<dt>{label}</dt><dd data-metric="{key}">{}</dd>"#,
            value.map_or_else(|| "Нет данных".to_string(), |v| v.to_string())
        )
        .expect("String");
    }
    body.push_str("</dl>");
    if metrics.evidence_invalidated {
        invalidated(body);
    }
}

pub(super) fn invalidated(body: &mut String) {
    body.push_str("<p data-evidence=\"invalidated\">Часть прежних данных удалена. Связанные исторические показатели теперь неизвестны и не заменяются нулями. Доступные измерения показаны отдельно.</p>");
}

pub(super) fn comparison(
    body: &mut String,
    prefix: &str,
    current: &BookDiscoveryMetrics,
    previous: &BookDiscoveryMetrics,
) {
    write!(body, r#"<details class="book-section-card audience-source-card" data-testid="{prefix}-comparison" data-measurement="legacy-consented">
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
        BookDiscoveryObservation::UnknownEvidence => {
            ("unknown-evidence", "Исторические данные неполны")
        }
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
    write!(body, r#"<section class="book-section-card audience-source-card" data-testid="{prefix}-{key}" data-period="{key}" data-measurement="legacy-consented">
        <h3>{label}</h3><p>С {} до {} UTC.</p>"#, escape_html(&period.from_day), escape_html(&period.until_day_exclusive)).expect("String");
    if period.includes_partial_today {
        body.push_str("<p>Включает незавершённый сегодняшний день.</p>");
    }
    metrics(body, &period.totals);
    counts(body, &period.counts);
    body.push_str("</section>");
}

pub(super) fn counts(body: &mut String, counts: &BookDiscoveryCounts) {
    body.push_str(
        "<h4>Состояние книг в сохранённой выборке</h4><dl data-measurement=\"legacy-consented\">",
    );
    for (key, label, value) in [
        (
            "unknown-evidence",
            "Исторические данные неполны",
            counts.unknown_evidence,
        ),
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
    write!(body, r#"<section class="book-section-card" data-testid="{prefix}-health"><h3>Обработка исторических данных</h3>"#).expect("String");
    if let Some(last_success) = &health.last_success_utc {
        write!(
            body,
            "<p>Последнее подтверждение прежнего обработчика: {}.</p>",
            time(last_success)
        )
        .expect("String");
    } else {
        body.push_str(
            "<p>Подтверждение прежнего обработчика не сохранено. Полнота истории неизвестна.</p>",
        );
    }
    body.push_str("<p>Эта отметка относится к прежней модели и не подтверждает текущий сбор.</p>");
    if health.rec_dirty || health.organic_dirty {
        body.push_str("<p>Оставшиеся индивидуальные записи ожидают удаления. Связанные итоги могут стать неизвестными.</p>");
    }
    body.push_str("</section>");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_book_keeps_independent_zero_and_omits_unknown_delta() {
        let current = BookDiscoveryMetrics {
            issued: Some(0),
            impressions: None,
            clicks: None,
            book_opens: None,
            starts: None,
            evidence_invalidated: true,
        };
        let previous = BookDiscoveryMetrics {
            issued: Some(2),
            impressions: Some(3),
            clicks: Some(2),
            book_opens: Some(1),
            starts: Some(1),
            evidence_invalidated: false,
        };
        let mut html = String::new();
        metrics(&mut html, &current);
        comparison(&mut html, "book", &current, &previous);
        assert!(html.contains("data-evidence=\"invalidated\""));
        assert!(html.contains("data-metric=\"issued\">0</dd>"));
        assert!(html.contains("data-metric=\"starts\">Нет данных</dd>"));
        assert!(html.contains("data-change=\"issued\">-2</dd>"));
        assert!(html.contains("data-change=\"starts\">Нет сравнения</dd>"));
        assert!(!html.contains("data-change=\"starts\">-1</dd>"));
        assert_eq!(
            observation(BookDiscoveryObservation::UnknownEvidence),
            ("unknown-evidence", "Исторические данные неполны")
        );
    }
}
