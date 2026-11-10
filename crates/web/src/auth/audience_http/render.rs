//! Only numerical evidence is rendered; no reader or book identifiers.
pub(super) mod book_returns;
pub(super) mod books;
mod evidence;
pub(super) mod group_returns;
pub(super) mod groups;
mod hours;
pub(super) mod returns;
mod sources;

use crate::pages::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_reading::services::audience::{AudienceCounts, AudiencePeriod, AudienceReport};
use tor_recommendations::services::audience::DiscoveryReport;
use tor_traffic_behavior::services::audience::FlowHourlyReport;

fn start(days: i32) -> String {
    let mut body = format!(
        "<section class=\"section\"><div class=\"container author-width\">\
         <p class=\"eyebrow\">Администратору сайта</p><h1>Аудитория</h1>\
         <p>Просмотры страниц, источники переходов к книгам и подтверждённое чтение. \
         Только общие числа, без истории отдельных читателей.</p>\
         <p><a class=\"button button--quiet\" data-testid=\"audience-books-link\" href=\"/admin/audience/books?days={days}\">Какие книги замечают и читают</a></p>\
         <p><a class=\"button button--quiet\" data-testid=\"audience-groups-link\" href=\"/admin/audience/groups?days={days}&amp;kind=genres\">Жанры и теги</a></p>\
         <p><a class=\"button button--quiet\" data-testid=\"audience-returns-link\" href=\"/admin/audience/returns?days={days}\">Возвращаются ли к чтению</a></p>\
         <form class=\"author-form\" method=\"get\" action=\"/admin/audience\">\
         <label for=\"audience-days\">Длительность периода</label>\
         <select id=\"audience-days\" name=\"days\">",
    );
    for (value, label) in [
        (1, "1 день"),
        (7, "7 дней"),
        (30, "30 дней"),
        (90, "90 дней"),
    ] {
        let selected = if days == value { " selected" } else { "" };
        write!(body, "<option value=\"{value}\"{selected}>{label}</option>").expect("String");
    }
    body.push_str(
        "</select><p><button class=\"button\" type=\"submit\">Показать</button></p></form>",
    );
    body
}

fn finish(mut body: String) -> Html<String> {
    body.push_str(
        "<section class=\"book-section-card\"><h2>Как считается чтение глав</h2>\
         <p>Начало и завершение — подтверждённые системой события чтения главы. \
         Повторные открытия и повторная отправка прогресса не создают новое первое событие.</p>\
         <p>Это число учётных записей с такими событиями, а не все посетители сайта и не число людей. \
         Анонимные посетители и повторное чтение уже учтённых глав здесь не измеряются. \
         По этой сводке нельзя определить возвращаемость читателей.</p>\
         <p>Начало и завершение одной главы могут попасть в разные периоды. \
         Соотношение этих чисел не является долей дочитанных глав. \
         Уникальные читатели за период вычисляются целиком, без сложения дневных значений.</p>\
         <p>Показаны только общие числа, без имён читателей, названий книг и истории отдельных пользователей.</p>\
         </section><p><a href=\"/me\">Вернуться в личный кабинет</a></p></div></section>",
    );
    layout(
        "Аудитория",
        "Закрытая сводка источников и подтверждённого чтения.",
        &body,
    )
}

pub(super) fn page(
    days: i32,
    reading: Option<&AudienceReport>,
    discovery: Option<&DiscoveryReport>,
    traffic: Option<&FlowHourlyReport>,
) -> Html<String> {
    let mut body = start(days);
    hours::section(&mut body, days, traffic);
    match discovery {
        Some(report) => sources::section(&mut body, report),
        None => body.push_str("<section data-testid=\"audience-sources-unavailable\"><h2>Источники первого чтения книги</h2><p role=\"alert\">Сводка источников временно недоступна. Нулевые значения не подставляются.</p></section>"),
    }
    body.push_str(
        "<h2>Подтверждённое чтение глав</h2>\
         <p>Зарегистрированные учётные записи с первым подтверждённым началом или завершением главы за период. \
         Один читатель учитывается один раз за весь период, даже если читает разные книги.</p>\
         <p>Сравниваются два соседних периода одинаковой длительности. Время указано в UTC: \
         начало включено, конец не включён. Текущий период заканчивается в момент расчёта.</p>",
    );
    let Some(report) = reading else {
        body.push_str("<p role=\"alert\">Сводка чтения глав временно недоступна. Данные не получены; нулевые значения не подставляются.</p>");
        return finish(body);
    };
    write!(
        body,
        "<p>Время расчёта: {}.</p>",
        time(&report.measured_at_utc)
    )
    .expect("String");
    period(&mut body, "current", "Текущий период", &report.current);
    period(&mut body, "previous", "Предыдущий период", &report.previous);
    match (
        report.current.counts.as_ref().filter(|_| report.current.available),
        report.previous.counts.as_ref().filter(|_| report.previous.available),
    ) {
        (Some(current), Some(previous)) => {
            body.push_str("<section class=\"book-section-card\" data-testid=\"audience-comparison\"><h2>Изменение к предыдущему периоду</h2><dl>");
            for (label, now, before) in [
                ("Учётных записей с событиями", current.unique_readers, previous.unique_readers),
                ("Подтверждённых начал глав", current.starts, previous.starts),
                ("Подтверждённых завершений глав", current.completions, previous.completions),
            ] {
                let change = i128::from(now) - i128::from(before);
                write!(body, "<dt>{label}</dt><dd>{change:+}</dd>").expect("String");
            }
            body.push_str("</dl><p>Разница относится только к сохранённым событиям выбранных периодов.</p></section>");
        }
        _ => body.push_str("<p data-testid=\"audience-comparison-unavailable\">Изменение не рассчитано: для одного из периодов нет измерения.</p>"),
    }
    finish(body)
}

fn time(value: &str) -> String {
    format!(
        "<time datetime=\"{}\">{} UTC</time>",
        escape_html(value),
        escape_html(&value.trim_end_matches('Z').replace('T', " "))
    )
}

fn period(body: &mut String, key: &str, title: &str, period: &AudiencePeriod) {
    write!(body, "<section class=\"book-section-card\" data-testid=\"audience-{key}\"><h2>{title}</h2><p>С {}<br>до {}.</p>", time(&period.from_utc), time(&period.until_utc)).expect("String");
    if !period.available {
        body.push_str("<p>За этот период измерение недоступно. Значения не подставляются.</p>");
    } else if let Some(counts) = &period.counts {
        counts_list(body, counts);
    } else {
        body.push_str("<p>За этот период нет сохранённых подтверждённых событий. Это не доказывает отсутствие читателей.</p>");
    }
    body.push_str("</section>");
}

fn counts_list(body: &mut String, counts: &AudienceCounts) {
    body.push_str("<dl>");
    for (key, label, value) in [
        (
            "readers",
            "Учётные записи с событиями за весь период",
            counts.unique_readers,
        ),
        ("starts", "Первые подтверждённые начала глав", counts.starts),
        (
            "started-readers",
            "Учётные записи с началом главы",
            counts.unique_started,
        ),
        (
            "completions",
            "Первые подтверждённые завершения глав",
            counts.completions,
        ),
        (
            "completed-readers",
            "Учётные записи с завершением главы",
            counts.unique_completed,
        ),
    ] {
        write!(
            body,
            "<dt>{label}</dt><dd data-metric=\"{key}\">{value}</dd>"
        )
        .expect("String");
    }
    body.push_str("</dl>");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AudienceReport {
        AudienceReport {
            days: 7,
            measured_at_utc: "2026-11-10T12:00:00Z".into(),
            current: AudiencePeriod {
                from_utc: "2026-11-03T12:00:00Z".into(),
                until_utc: "2026-11-10T12:00:00Z".into(),
                available: true,
                counts: Some(AudienceCounts {
                    starts: 9,
                    completions: 12,
                    unique_readers: 5,
                    unique_started: 3,
                    unique_completed: 4,
                }),
            },
            previous: AudiencePeriod {
                from_utc: "2026-10-27T12:00:00Z".into(),
                until_utc: "2026-11-03T12:00:00Z".into(),
                available: true,
                counts: Some(AudienceCounts {
                    starts: 5,
                    completions: 6,
                    unique_readers: 4,
                    unique_started: 4,
                    unique_completed: 2,
                }),
            },
        }
    }

    #[test]
    fn periods_use_report_distinct_counts_without_inventing_retention_or_conversion() {
        let html = page(7, Some(&sample()), None, None).0;
        assert!(html.contains("data-metric=\"readers\">5</dd>"));
        assert!(html.contains("<dd>+1</dd>"));
        assert!(html.contains("datetime=\"2026-11-03T12:00:00Z\""));
        assert!(html.contains("конец не включён"));
        assert!(html.contains("нельзя определить возвращаемость"));
        assert!(html.contains("не является долей дочитанных глав"));
        assert!(html.contains("value=\"7\" selected"));
        assert!(!html.contains("user_id"));
        assert!(!html.contains("book_id"));
    }

    #[test]
    fn absent_events_do_not_turn_into_zero_readers_or_comparison() {
        let mut report = sample();
        report.previous.counts = None;
        let html = page(7, Some(&report), None, None).0;
        assert!(html.contains("Это не доказывает отсутствие читателей"));
        assert!(html.contains("audience-comparison-unavailable"));
        assert!(!html.contains("data-metric=\"readers\">0"));
        report.current.available = false;
        assert!(!page(7, Some(&report), None, None)
            .0
            .contains("data-metric=\"readers\""));
        assert!(page(30, None, None, None).0.contains("role=\"alert\""));
    }

    #[test]
    fn timestamp_text_and_attribute_are_escaped() {
        let mut report = sample();
        report.current.from_utc = "\"><script>bad()</script>".into();
        let html = page(7, Some(&report), None, None).0;
        assert!(!html.contains("<script>bad()"));
        assert!(html.contains("&quot;&gt;&lt;script&gt;"));
    }
}
