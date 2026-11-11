//! Separate chapter-event clock view; never combines C events with HTML traffic.
use crate::pages::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_reading::services::audience::time::{
    ReadingTimeCell, ReadingTimeCounts, ReadingTimePeriod, ReadingTimeReport,
};

const WEEKDAYS: [&str; 7] = ["Пн", "Вт", "Ср", "Чт", "Пт", "Сб", "Вс"];

pub(super) fn page(days: i32, zone: &str, report: Option<&ReadingTimeReport>) -> Html<String> {
    let navigation = crate::flow_navigation::links(days);
    let mut body = format!(
        "<section class=\"section\"><div class=\"container author-width\"><p class=\"eyebrow\">Администратору сайта</p><h1>Подтверждённое чтение по времени</h1>{navigation}"
    );
    body.push_str("<p data-testid=\"reading-time-scope\">Все сохранившиеся подтверждённые начала и завершения глав за выбранные часы. Следующая глава прежнего читателя учитывается; повтор того же подтверждения не добавляет событие. Это не число первых пар «аккаунт–книга», людей, прочитанных книг или длительность чтения.</p>");
    super::period_form(&mut body, days, zone, true);
    body.push_str("<section class=\"book-section-card\"><h2>Что означают числа</h2><p>Два соседних периода по 24 часа × число дней. Границы — закрытые часы UTC: начало включено, конец не включён. Текущий незавершённый час исключён. Часовой пояс меняет местные подписи и распределение по часам и дням недели, сохраняя границы и общие числа.</p><p>Нуль означает отсутствие сохранившихся подтверждённых записей в этом окне. Он не доказывает отсутствие чтения. Неизвестное или неверное подтверждение не заменяется нулём; число людей и полнота всей истории не оцениваются.</p><p>Учитываются исторически разрешённые записи чтения независимо от нынешней публичности книги. Имена, книги и история отдельных читателей не выводятся. Начало и завершение главы могут попасть в разные часы; их соотношение не является долей дочитывания.</p></section>");
    let Some(report) = report else {
        body.push_str("<section class=\"book-section-card\" data-testid=\"reading-time-unavailable\"><h2>Источник чтения недоступен</h2><p role=\"alert\">Данные не получены. Значения и сравнение не рассчитаны.</p></section>");
        return finish(body);
    };
    write!(
        body,
        "<p data-testid=\"reading-time-anchor\" data-until=\"{}\">Время расчёта: {}. Часовой пояс: <strong>{}</strong>.</p>",
        report.until_epoch,
        super::time(&report.measured_at_utc),
        escape_html(&report.time_zone)
    ).expect("String");
    body.push_str("<div class=\"audience-source-grid\">");
    period(&mut body, "current", "Текущий период", &report.current);
    period(&mut body, "previous", "Предыдущий период", &report.previous);
    body.push_str("</div>");
    match (&report.current.totals, &report.previous.totals) {
        (Some(current), Some(previous)) => {
            write!(body, "<section class=\"book-section-card\" data-testid=\"reading-time-comparison\"><h2>Изменение сохранённых записей</h2><dl><dt>Подтверждённые начала глав</dt><dd data-metric=\"starts-change\">{:+}</dd><dt>Подтверждённые завершения глав</dt><dd data-metric=\"completions-change\">{:+}</dd></dl><p>Разница двух равных закрытых периодов; не изменение числа людей и не причинный эффект продвижения.</p></section>",
                i128::from(current.starts)-i128::from(previous.starts),
                i128::from(current.chapter_completions)-i128::from(previous.chapter_completions)).expect("String");
        }
        _ => body.push_str("<p data-testid=\"reading-time-comparison-unknown\">Сравнение не рассчитано: один из периодов содержит неизвестное подтверждение.</p>"),
    }
    profile(
        &mut body, "hour", "Часы суток",
        &report.current.by_hour, &report.previous.by_hour,
    );
    profile(
        &mut body, "weekday", "Дни недели",
        &report.current.by_weekday, &report.previous.by_weekday,
    );
    details(&mut body, "current", "Текущий период", &report.current);
    details(&mut body, "previous", "Предыдущий период", &report.previous);
    finish(body)
}

fn finish(mut body: String) -> Html<String> {
    body.push_str("<p><a href=\"/me\">Вернуться в личный кабинет</a></p></div></section>");
    layout("Чтение по времени", "Закрытая сводка подтверждённых записей чтения по времени.", &body)
}

fn number(value: Option<i64>) -> String {
    value.map_or_else(|| "Неизвестно".into(), |value| value.to_string())
}

fn counts(body: &mut String, value: Option<&ReadingTimeCounts>) {
    write!(body, "<span data-metric=\"chapter-starts\">{}</span> / <span data-metric=\"chapter-completions\">{}</span>",
        number(value.map(|v|v.starts)),
        number(value.map(|v|v.chapter_completions))).expect("String");
}

fn period(body: &mut String, key: &str, title: &str, value: &ReadingTimePeriod) {
    write!(body, "<section class=\"book-section-card\" data-testid=\"reading-time-{key}\" data-from=\"{}\" data-until=\"{}\"><h2>{title}</h2><p>{} — {}.</p><p>Закрытых часов UTC: <span data-metric=\"utc-hours\">{}</span>.</p><p>Подтверждённые начала / завершения глав: ",
        value.from_epoch,value.until_epoch,super::time(&value.from_utc),super::time(&value.until_utc),value.hours.len()).expect("String");
    counts(body, value.totals.as_ref());
    body.push_str(".</p>");
    if value.totals.is_none() {
        body.push_str("<p data-testid=\"reading-time-period-unknown\">Есть записи без подходящего подтверждения. Общие числа периода неизвестны; известные часы показаны отдельно.</p>");
    }
    body.push_str("</section>");
}

fn cell(body: &mut String, key: &str, value: &ReadingTimeCell) {
    write!(body,"<td data-period=\"{key}\" data-utc-hours=\"{}\">",value.utc_hours).expect("String");
    if value.utc_hours == 0 {
        body.push_str("<span data-testid=\"reading-time-no-slot\">Нет такого часа в окне</span>");
    } else {
        counts(body,value.counts.as_ref());
        write!(body,"<br><small>Часов UTC: {}</small>",value.utc_hours).expect("String");
    }
    body.push_str("</td>");
}

fn profile(body: &mut String, key: &str, title: &str, current: &[ReadingTimeCell], previous: &[ReadingTimeCell]) {
    write!(body,"<section class=\"book-section-card\"><h2>{title}</h2><p>Начала / завершения глав суммируются за весь выбранный период. Число часов UTC показывает, сколько часов попало в местную группу; это не среднее и не длительность чтения.</p><div class=\"audience-hours-scroll\" tabindex=\"0\" role=\"region\" aria-label=\"{title}\"><table class=\"audience-hours-table\" data-testid=\"reading-time-{key}-profile\"><thead><tr><th scope=\"col\">{title}</th><th scope=\"col\">Текущий: начала / завершения</th><th scope=\"col\">Предыдущий: начала / завершения</th></tr></thead><tbody>").expect("String");
    for (index,(current,previous)) in current.iter().zip(previous).enumerate() {
        let label = if key == "hour" {format!("{index:02}:00")} else {WEEKDAYS[index].to_owned()};
        write!(body,"<tr data-reading-{key}=\"{index}\"><th scope=\"row\">{label}</th>").expect("String");
        cell(body,"current",current);
        cell(body,"previous",previous);
        body.push_str("</tr>");
    }
    body.push_str("</tbody></table></div></section>");
}

fn details(body: &mut String, key: &str, title: &str, value: &ReadingTimePeriod) {
    write!(body,"<details class=\"book-section-card\" data-testid=\"reading-time-{key}-details\"><summary>{title}: последние 168 закрытых часов</summary><p>Общие числа и распределения выше включают весь период. Повторяющийся местный час при смене времени остаётся отдельными строками с разными UTC и сдвигами.</p><div class=\"audience-hours-scroll\" tabindex=\"0\" role=\"region\" aria-label=\"{title}: часы\"><table class=\"audience-hours-table\" data-testid=\"reading-time-{key}-hours\"><thead><tr><th scope=\"col\">Местное начало часа</th><th scope=\"col\">Сдвиг к UTC</th><th scope=\"col\">Начала / завершения глав</th></tr></thead><tbody>").expect("String");
    for hour in value.hours.iter().skip(value.hours.len().saturating_sub(168)) {
        write!(body,"<tr data-reading-epoch=\"{}\" data-local-hour=\"{}\" data-weekday=\"{}\" data-offset-minutes=\"{}\"><th scope=\"row\">{}</th><td>{:+} мин</td><td>",
            hour.epoch,hour.local_hour,hour.weekday,hour.offset_minutes,escape_html(&hour.local_label),hour.offset_minutes).expect("String");
        counts(body,hour.counts.as_ref());
        body.push_str("</td></tr>");
    }
    body.push_str("</tbody></table></div></details>");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retained_zero_unknown_and_absent_local_slot_are_distinct() {
        let mut html=String::new();
        counts(&mut html,Some(&ReadingTimeCounts{starts:0,chapter_completions:0}));
        assert!(html.contains("data-metric=\"chapter-starts\">0</span>"));
        html.clear();
        counts(&mut html,None);
        assert!(html.contains("Неизвестно"));
        assert!(!html.contains(">0<"));
        html.clear();
        cell(&mut html,"current",&ReadingTimeCell{utc_hours:0,counts:None});
        assert!(html.contains("reading-time-no-slot"));
        assert!(!html.contains("data-metric="));
        html.clear();
        cell(&mut html,"current",&ReadingTimeCell{utc_hours:2,counts:None});
        assert!(html.contains("data-utc-hours=\"2\""));
        assert!(html.contains("Неизвестно"));
        assert!(!html.contains("reading-time-no-slot"));
    }

    #[test]
    fn unavailable_source_keeps_native_selection_without_numeric_fallback() {
        let html=page(90,"Europe/Amsterdam",None).0;
        assert!(html.contains("value=\"reading_hours\" selected"));
        assert!(html.contains("value=\"90\" selected"));
        assert!(html.contains("value=\"Europe/Amsterdam\" selected"));
        assert!(html.contains("reading-time-unavailable"));
        assert!(!html.contains("data-metric="));
        assert!(!html.contains("audience-observed"));
        assert!(html.contains("Следующая глава прежнего читателя учитывается"));
        assert!(html.contains("Текущий незавершённый час исключён"));
    }
}
