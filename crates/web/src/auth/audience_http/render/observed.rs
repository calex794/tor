//! FLOW-04: observation-hour counts; no individual navigation or tracking UI.
mod health;
use crate::pages::escape_html;
use std::fmt::Write;
use tor_traffic_behavior::domain::html_health::HtmlHealthReport;
use tor_traffic_behavior::services::observed_audience::{ObservedAudienceReport, ObservedPeriod};

const SURFACES: [&str; 4] = ["Главная", "Каталог", "Страницы книг", "Читалка"];
const SOURCES: [&str; 4] = [
    "Внешний поиск",
    "Внешняя ссылка",
    "Платный внешний канал",
    "Прямой или неизвестный",
];
const WEEKDAYS: [&str; 7] = ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"];

fn number(value: Option<i64>) -> String {
    value.map_or_else(|| "Нет измерения".into(), |n| n.to_string())
}

pub(super) fn section(
    body: &mut String,
    report: Option<&ObservedAudienceReport>,
    quality: Option<&HtmlHealthReport>,
) {
    health::section(body, report, quality);
    let Some(report) = report else {
        body.push_str("<section class=\"book-section-card\" data-testid=\"audience-observed-unavailable\"><h2>Просмотры по времени ответа сервера</h2><p role=\"alert\">Отчет по времени просмотра недоступен. Данные не получены; нули не подставляются.</p></section>");
        return;
    };
    body.push_str("<section id=\"audience-observed-hours\" data-testid=\"audience-observed\"><h2>Просмотры по времени ответа сервера</h2><p>Успешные ответы публичных HTML-страниц, не люди, не визиты и не подтвержденное чтение. Час берется из часов сервера до очереди и записи в базу.</p><p>Новая статистика не восстанавливает историю из старых пакетов. Пропуск данных не равен нулю. Повторные открытия считаются повторно.</p><p>Сравниваются два соседних интервала завершенных часов. Текущий незавершенный час не включен. Часовой пояс меняет отображение, не границы периода.</p>");
    write!(
        body,
        "<p>Часовой пояс: <strong>{}</strong>.</p>",
        escape_html(&report.time_zone)
    )
    .expect("String");
    body.push_str("<div class=\"audience-source-grid\">");
    let quality = quality.filter(|proof| proof.until_epoch == report.current.until_epoch);
    let current_complete =
        health::complete(quality.map(|proof| proof.current), report.current.hours.len());
    let previous_complete =
        health::complete(quality.map(|proof| proof.previous), report.previous.hours.len());
    period(body, "current", "Текущий период", &report.current, current_complete);
    period(body, "previous", "Предыдущий период", &report.previous, previous_complete);
    body.push_str("</div><p>Это категории недоверенного заголовка запроса, не доказанный первый вход из поиска. Внутренний поиск и рекомендации здесь не подменяют внешние каналы.</p>");
    write!(body,"<p>Отброшено при обработке новых пакетов: <span data-metric=\"observed-discards\">{}</span>. По времени обработки за текущий период: неверные часы, предел памяти или истекший срок. Потери до очереди и при остановке процесса в это число не входят.</p>",report.persisted_discards).expect("String");
    if report.persisted_discards > 0 {
        body.push_str("<p role=\"alert\" data-testid=\"audience-observed-loss\">Обнаружены потери при обработке. Показатели не являются полным учетом трафика.</p>");
    }
    heatmap(body, &report.current, current_complete);
    body.push_str(
        "<p>Таблица: последние 168 часов. Сводка и дни недели охватывают весь период.</p>",
    );
    body.push_str("<details class=\"book-section-card\"><summary>Последние 168 часов по времени просмотра</summary><p>При переходе на зимнее время повторяющиеся местные часы сохраняются отдельными строками с разным сдвигом к UTC.</p><div class=\"audience-hours-scroll\" tabindex=\"0\" role=\"region\" aria-label=\"Просмотры по времени ответа сервера\"><table class=\"audience-hours-table\" data-testid=\"audience-observed-table\"><thead><tr><th scope=\"col\">Начало часа</th><th scope=\"col\">Сдвиг к UTC</th>");
    for label in SURFACES {
        write!(body, "<th scope=\"col\">{label}</th>").expect("String");
    }
    body.push_str("</tr></thead><tbody>");
    for hour in report
        .current
        .hours
        .iter()
        .skip(report.current.hours.len().saturating_sub(168))
    {
        write!(
            body,
            "<tr data-observed-hour=\"{}\"><th scope=\"row\">{}</th><td>{:+} мин</td>",
            hour.epoch,
            escape_html(&hour.local_label),
            hour.offset_minutes
        )
        .expect("String");
        for value in hour.views {
            write!(body, "<td>{}</td>", number(value.or(current_complete.then_some(0)))).expect("String");
        }
        body.push_str("</tr>");
    }
    body.push_str("</tbody></table></div></details></section>");
}

fn period(body: &mut String, key: &str, title: &str, value: &ObservedPeriod, complete: bool) {
    write!(body,"<section class=\"book-section-card\" data-testid=\"audience-observed-{key}\"><h3>{title}</h3><p>{} &mdash; {}.</p><p>Сохраненные просмотры: <strong data-metric=\"observed-total\">{}</strong>.</p><p>Часов с записями: <span data-metric=\"observed-hours\">{}</span> из {}.</p><dl>",
        super::time(&value.from_utc),super::time(&value.until_utc),number(value.saved_total.or(complete.then_some(0))),
        value.recorded_hours,value.hours.len()).expect("String");
    for (index, label) in SURFACES.iter().enumerate() {
        write!(
            body,
            "<dt>{label}</dt><dd data-observed-surface=\"{index}\">{}</dd>",
            number(value.surfaces[index].or(complete.then_some(0)))
        )
        .expect("String");
    }
    body.push_str("</dl><h4>Подсказки источника запроса</h4><dl>");
    for (index, label) in SOURCES.iter().enumerate() {
        write!(
            body,
            "<dt>{label}</dt><dd data-observed-source=\"{index}\">{}</dd>",
            number(value.sources[index].or(complete.then_some(0)))
        )
        .expect("String");
    }
    body.push_str("</dl></section>");
}

fn heatmap(body: &mut String, period: &ObservedPeriod, complete: bool) {
    body.push_str("<h3>Дни недели и часы</h3><p>Сохраненные просмотры; часы с записями / часы в интервале. Полноту сбора это не доказывает.</p><div class=\"audience-hours-scroll\" tabindex=\"0\" role=\"region\" aria-label=\"Дни недели и часы\"><table class=\"audience-hours-table\" data-testid=\"audience-observed-week\"><thead><tr><th scope=\"col\">Дни недели и часы</th>");
    for hour in 0..24 {
        write!(body, "<th scope=\"col\">{hour:02}:00</th>").expect("String");
    }
    body.push_str("</tr></thead><tbody>");
    for (day, label) in WEEKDAYS.iter().enumerate() {
        write!(body, "<tr><th scope=\"row\">{label}</th>").expect("String");
        for hour in 0..24 {
            write!(
                body,
                "<td>{}<br><small>{}/{}</small></td>",
                number(period.weekday_hour[day][hour].or((complete && period.expected_slots[day][hour] > 0).then_some(0))),
                period.recorded_slots[day][hour],
                period.expected_slots[day][hour]
            )
            .expect("String");
        }
        body.push_str("</tr>");
    }
    body.push_str("</tbody></table></div>");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> ObservedPeriod {
        ObservedPeriod {
            from_epoch: 0,
            until_epoch: 3_600,
            from_utc: "2026-11-11T00:00:00Z".into(),
            until_utc: "2026-11-11T01:00:00Z".into(),
            hours: vec![],
            surfaces: [Some(0), None, None, None],
            sources: [None, None, None, Some(0)],
            saved_total: Some(0),
            recorded_hours: 1,
            weekday_hour: [[None; 24]; 7],
            recorded_slots: [[0; 24]; 7],
            expected_slots: [[0; 24]; 7],
        }
    }

    #[test]
    fn absence_zero_and_failure_have_distinct_markup() {
        let mut html = String::new();
        section(&mut html, None, None);
        assert!(html.contains("audience-observed-unavailable"));
        assert!(!html.contains("data-metric="));
        let report = ObservedAudienceReport {
            days: 1,
            time_zone: "UTC".into(),
            measured_at_utc: "now".into(),
            current: empty(),
            previous: empty(),
            persisted_discards: 2,
        };
        html.clear();
        section(&mut html, Some(&report), None);
        assert!(html.contains("data-observed-surface=\"0\">0</dd>"));
        assert!(html.contains("data-observed-surface=\"1\">Нет измерения</dd>"));
        assert!(html.contains("audience-observed-loss"));
    }

    #[test]
    fn report_text_cannot_inject_html_or_make_unknown_counts_numeric() {
        let mut report = ObservedAudienceReport {
            days: 7,
            time_zone: "\"><script>bad()</script>".into(),
            measured_at_utc: "now".into(),
            current: empty(),
            previous: empty(),
            persisted_discards: 0,
        };
        report.current.from_utc = report.time_zone.clone();
        let mut html = String::new();
        section(&mut html, Some(&report), None);
        assert!(!html.contains("<script>bad()"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("audience-observed-loss"));
    }
}
