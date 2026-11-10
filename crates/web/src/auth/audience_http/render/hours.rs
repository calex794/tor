//! Read-only, non-identifying hourly evidence. Missing buckets are never zero-filled.
mod chart;

use super::time;
use crate::pages::escape_html;
use std::fmt::Write;
use tor_traffic_behavior::services::audience::{FlowHourlyPeriod, FlowHourlyReport};

const SURFACES: [(&str, &str); 4] = [
    ("home", "Главная"),
    ("catalog", "Каталог"),
    ("book", "Страницы книг"),
    ("reader", "Читалка"),
];

pub(super) fn section(body: &mut String, days: i32, report: Option<&FlowHourlyReport>) {
    if !matches!(days, 1 | 7) {
        body.push_str(
            "<section class=\"book-section-card\" data-testid=\"audience-hours-unsupported\">\
             <h2>Просмотры по часам</h2><p>Сравнение просмотров доступно за сутки и неделю. \
             Часовые данные хранятся 30 дней: двух полных периодов по 30 или 90 дней здесь нет.</p>\
             <p><a href=\"/admin/audience?days=7\">Показать неделю</a>. \
             Источники и подтверждённое чтение за выбранный период доступны ниже.</p></section>",
        );
        return;
    }
    let Some(report) = report else {
        body.push_str(
            "<section class=\"book-section-card\" data-testid=\"audience-hours-unavailable\">\
             <h2>Просмотры по часам</h2><p role=\"alert\">Сводка просмотров временно недоступна. \
             Данные не получены; нулевые значения не подставляются.</p></section>",
        );
        return;
    };
    body.push_str(
        "<section data-testid=\"audience-hours\"><h2>Просмотры по часам</h2>\
         <p>Сохранённые открытия главной, каталога, страниц книг и читалки. \
         Повторное открытие считается ещё одним просмотром. Это не число людей \
         и не подтверждение чтения.</p>",
    );
    write!(body, "<p>Сравниваются последние {} завершённых часов и предыдущий равный период. \
        Текущий незавершённый час исключён. Все часы указаны в UTC; начало включено, конец не включён.</p>",
        report.current.hours.len()).expect("String");
    body.push_str(
        "<p>Час соответствует записи пакета в базу. При задержке записи он может \
         отличаться от времени посещения.</p>",
    );
    chart::render(body, report);
    body.push_str("<div class=\"audience-source-grid\">");
    period(body, "current", "Текущий период", &report.current);
    period(body, "previous", "Предыдущий период", &report.previous);
    body.push_str(
        "</div><p>Начала визитов учитываются только при действующем согласии. \
         Это наблюдения в памяти одного процесса, а не число уникальных людей \
         или всех визитов сайта. Первые 30 минут после перезапуска, потери \
         сведений о сеансе или отзыва согласия сомнительные новые визиты не \
         засчитываются. При нескольких серверах общий учёт не подтверждён. \
         Старые результаты до этого ограничения могут включать повторы. \
         Личный кабинет здесь не учитывается.</p>\
         <p>В часах без записей нет измерения. Это не доказывает отсутствие просмотров. \
         Час с записью тоже не гарантирует полный сбор: часть запросов могла быть потеряна \
         до сохранения. Количество часов с записями не является оценкой полноты.</p>",
    );
    table(body, report);
    body.push_str(
        "<details class=\"book-section-card\"><summary>Как считаются просмотры</summary>\
         <p>Учтены успешные открытия известных HTML-страниц. Служебные запросы, ошибки, \
         перенаправления и распознанные по браузерной подписи боты исключаются. \
         Этот фильтр не доказывает, что каждый оставшийся запрос сделал человек.</p>\
         <p>Здесь только обезличенные суммы четырёх публичных категорий. \
         Личный кабинет и история отдельных посетителей не показываются. \
         Отказ от поведенческого учёта не останавливает обезличенный подсчёт страниц.</p>\
         <p>Время расчёта: ",
    );
    body.push_str(&time(&report.measured_at_utc));
    body.push_str(".</p></details></section>");
}

fn period(body: &mut String, key: &str, label: &str, period: &FlowHourlyPeriod) {
    write!(body, "<section class=\"book-section-card audience-hour-period\" data-testid=\"audience-hours-{key}\">\
        <h3>{label}</h3><p>С {}<br>до {}.</p>", time(&period.from_utc), time(&period.until_utc))
        .expect("String");
    for (index, (key, label)) in SURFACES.iter().enumerate() {
        let views = period.saved_views[index]
            .map_or_else(|| "Нет данных".to_owned(), |value| value.to_string());
        let opted = period.saved_opted_in_requests[index]
            .map_or_else(|| "Нет данных".to_owned(), |value| value.to_string());
        let visits = period.observed_visit_starts[index]
            .map_or_else(|| "Нет данных".to_owned(), |value| value.to_string());
        write!(
            body,
            "<div class=\"audience-hour-total\" data-surface=\"{key}\"><h4>{label}</h4>\
            <p><strong data-metric=\"saved-views\">{views}</strong> — сохранённые просмотры</p>\
            <p><strong data-metric=\"opted-in-requests\">{opted}</strong> — запросы с согласием</p>\
            <p><strong data-metric=\"observed-visit-starts\">{visits}</strong> — наблюдаемые начала визитов</p>\
            <p>Часов с записями: <span data-metric=\"recorded-hours\">{}</span> из {}.</p></div>",
            period.recorded_hours[index],
            period.hours.len()
        )
        .expect("String");
    }
    body.push_str("</section>");
}

fn table(body: &mut String, report: &FlowHourlyReport) {
    body.push_str(
        "<details class=\"book-section-card\"><summary>Все часы и категории</summary>\
         <p>В каждой паре сначала текущий период, затем предыдущий. \
         На узком экране таблицу можно прокрутить по горизонтали.</p>\
         <div class=\"audience-hours-scroll\" tabindex=\"0\" role=\"region\" aria-label=\"Просмотры по каждому часу\">\
         <table class=\"audience-hours-table\" data-testid=\"audience-hours-table\">\
         <caption>Сохранённые просмотры. «—» означает отсутствие сохранённого измерения, а не ноль.</caption>\
         <thead><tr><th rowspan=\"2\" scope=\"col\">Начало часа UTC<br>Текущий / предыдущий</th>",
    );
    for (_, label) in SURFACES {
        write!(body, "<th colspan=\"2\" scope=\"colgroup\">{label}</th>").expect("String");
    }
    body.push_str("</tr><tr>");
    for _ in SURFACES {
        body.push_str("<th scope=\"col\">Текущий</th><th scope=\"col\">Предыдущий</th>");
    }
    body.push_str("</tr></thead><tbody>");
    for (current, previous) in report.current.hours.iter().zip(&report.previous.hours) {
        write!(
            body,
            "<tr data-hour=\"{}\"><th scope=\"row\">{}<br>{}</th>",
            current.hour_epoch,
            hour(&current.hour_utc),
            hour(&previous.hour_utc)
        )
        .expect("String");
        for (index, (key, _)) in SURFACES.iter().enumerate() {
            for (period, value) in [
                ("current", current.views[index]),
                ("previous", previous.views[index]),
            ] {
                let value = value.map_or_else(|| "—".to_owned(), |value| value.to_string());
                write!(
                    body,
                    "<td data-surface=\"{key}\" data-period=\"{period}\">{value}</td>"
                )
                .expect("String");
            }
        }
        body.push_str("</tr>");
    }
    body.push_str("</tbody></table></div></details>");
}

fn hour(value: &str) -> String {
    format!(
        "<time datetime=\"{}\">{}</time>",
        escape_html(value),
        short_hour(value)
    )
}

fn short_hour(value: &str) -> String {
    match (value.get(8..10), value.get(5..7), value.get(11..16)) {
        (Some(day), Some(month), Some(hour)) if value.is_ascii() => {
            escape_html(&format!("{day}.{month} {hour}"))
        }
        _ => escape_html(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_and_unsupported_are_not_an_empty_successful_week() {
        let mut body = String::new();
        section(&mut body, 7, None);
        assert!(body.contains("audience-hours-unavailable"));
        assert!(body.contains("role=\"alert\""));
        assert!(!body.contains("data-metric="));
        body.clear();
        section(&mut body, 30, None);
        assert!(body.contains("audience-hours-unsupported"));
        assert!(body.contains("/admin/audience?days=7"));
        assert!(!body.contains("role=\"alert\""));
    }

    #[test]
    fn missing_category_and_measured_zero_are_distinct_and_safe() {
        let period_data = FlowHourlyPeriod {
            from_epoch: 0,
            until_epoch: 3600,
            from_utc: "2026-11-09T00:00:00Z".into(),
            until_utc: "2026-11-09T01:00:00Z".into(),
            hours: vec![],
            saved_views: [Some(0), None, Some(2), None],
            saved_opted_in_requests: [Some(0), None, Some(1), None],
            observed_visit_starts: [Some(0), None, Some(1), None],
            recorded_hours: [1, 0, 1, 0],
        };
        let mut body = String::new();
        period(&mut body, "current", "Текущий период", &period_data);
        assert!(body.contains("data-metric=\"saved-views\">0</strong>"));
        assert!(body.contains("data-metric=\"opted-in-requests\">0</strong>"));
        assert!(body.contains("data-metric=\"observed-visit-starts\">1</strong>"));
        assert_eq!(
            body.matches("data-metric=\"saved-views\">Нет данных")
                .count(),
            2
        );
        assert!(hour("\"><script>alert(1)</script>").contains("&quot;&gt;&lt;script&gt;"));
        assert!(!hour("\"><script>alert(1)</script>").contains("<script>"));
        assert_eq!(short_hour("2026-11-09T22:00:00Z"), "09.11 22:00");
    }
}
