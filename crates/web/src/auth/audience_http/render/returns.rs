//! Aggregate return cohorts. Pending and missing observations are never zero returns.
use super::time;
use crate::pages::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_reading::services::returns::{
    ReadingReturnsMetric, ReadingReturnsPeriod, ReadingReturnsReport,
};

pub(crate) fn page(days: i32, report: Option<&ReadingReturnsReport>) -> Html<String> {
    let mut body = String::from(
        r#"<section class="section"><div class="container audience-books-width">
        <p class="eyebrow">Администратору сайта</p><h1>Возвращаются ли к чтению</h1>
        <p>Подтверждённые возвращения на 1-й, 7-й и 30-й день.
        Чтение может продолжиться в любой разрешённой книге.</p>
        <p>Считаются наблюдения за учётными записями в период согласия.
        После отзыва и нового согласия начинается отдельное наблюдение.</p>"#,
    );
    write!(
        body,
        r#"<p><a href="/admin/audience?days={days}">Вернуться в «Аудиторию»</a></p>"#
    )
    .expect("String");
    filters(&mut body, days);
    match report {
        Some(report) => report_body(&mut body, report),
        None => body.push_str(
            r#"<section class="book-section-card" data-testid="audience-returns-unavailable">
            <h2>Отчёт временно недоступен</h2><p role="alert">Данные не получены.
            Нулевые значения не подставляются. Повторите запрос немного позже.</p></section>"#,
        ),
    }
    body.push_str(
        r#"<details class="book-section-card audience-books-help"><summary>Как считается возвращение</summary>
        <p>Начало наблюдения за учётной записью — первый принятый положительный прогресс
        после включения этого измерения и в текущем согласии. Это не дата регистрации
        и не обязательно первое чтение за всё время. История до начала сбора не восстанавливается.</p>
        <p>Возвращение на 7-й день означает новый принятый прогресс ровно в календарный день UTC
        через семь дней после начала наблюдения. Так же считаются 1-й и 30-й дни.
        Это не любое чтение в течение недели или месяца.</p>
        <p>В расчёт доли входят только записи, для которых целевой день полностью закончился
        и измерение было непрерывным. Например, если нужный день — сегодня, результат ещё ожидается.
        Для коротких периодов большинство поздних возвращений пока нельзя оценить.</p>
        <p>Повторные запросы, несколько глав и книг в один день не увеличивают дневной факт.
        Открытие страницы, повторное чтение того же места и отклонённое сохранение прогресса
        не подтверждают возвращение. Анонимное чтение и учётные записи без согласия не измеряются.</p>
        <p>Для купленной главы из защищённого хранилища день подтверждается проверенным
        завершением главы. Промежуточная позиция сама по себе в этот отчёт не входит.</p>
        <p>За один период согласия учётная запись участвует в наблюдении один раз. После отзыва и нового согласия
        наблюдение начинается заново; это не доказательство появления нового человека.
        При разрыве сбора затронутые результаты отмечаются как неизвестные.</p>
        <p>Подробные дневные записи хранятся до 30 суток. Дата начала и три признака возвращения
        сохраняются до завершения 30-го дня, максимум 31 сутки, затем остаются только безличные
        итоги до 180 дней. Признак уже проведённого наблюдения без даты предотвращает повторный учёт.
        Отзыв согласия удаляет личное состояние; прежние безличные итоги могут сохраняться.</p>
        <p>Отчёт показывает общие числа. Здесь нет имён, личных списков и истории отдельных книг.</p>
        </details><p><a href="/me">Вернуться в личный кабинет</a></p></div></section>"#,
    );
    layout(
        "Возвращаются ли к чтению — Аудитория",
        "Закрытая сводка подтверждённых возвращений к чтению.",
        &body,
    )
}

fn filters(body: &mut String, days: i32) {
    body.push_str(
        r#"<form class="author-form audience-books-filters" id="audience-returns-filters"
        method="get" action="/admin/audience/returns"><div>
        <label for="returns-days">Когда началось наблюдение за читателями</label>
        <select id="returns-days" name="days">"#,
    );
    for (value, label) in [
        (1, "1 день"),
        (7, "7 дней"),
        (30, "30 дней"),
        (90, "90 дней"),
    ] {
        let selected = if days == value { " selected" } else { "" };
        write!(
            body,
            r#"<option value="{value}"{selected}>{label}</option>"#
        )
        .expect("String");
    }
    body.push_str(
        r#"</select></div><p class="audience-books-filter-actions">
        <button class="button" type="submit">Показать</button></p></form>"#,
    );
}

fn report_body(body: &mut String, report: &ReadingReturnsReport) {
    write!(
        body,
        r#"<section data-testid="audience-returns" data-healthy="{}">
        <p>Время расчёта: {}.</p>
        <p>Сравниваются две группы по дате начала наблюдения: текущий и предыдущий периоды
        одинаковой длительности. Дни указаны в UTC; начало включено, конец не включён.
        Текущий период включает сегодняшний день.</p>"#,
        report.collection_healthy,
        time(&report.measured_at_utc)
    )
    .expect("String");
    let Some(start) = report.observation_started_at_utc.as_ref() else {
        body.push_str(
            r#"<p class="book-section-card" data-testid="audience-returns-not-started">
            Измерение ещё не началось. Сохранённые места и первые чтения глав не заменяют
            историю дней. Значения и доли пока не рассчитаны.</p></section>"#,
        );
        return;
    };
    write!(
        body,
        r#"<p data-testid="audience-returns-observation-start">Начало измерения: {}.</p>"#,
        time(start)
    )
    .expect("String");
    if let Some(last) = &report.last_observed_at_utc {
        write!(
            body,
            r#"<p data-testid="audience-returns-last-observed">Состояние сбора проверено: {}.</p>"#,
            time(last)
        )
        .expect("String");
    }
    if !report.collection_healthy {
        body.push_str(
            r#"<p class="book-section-card" role="status" data-testid="audience-returns-collection-gap">
            Сбор сейчас выключен или его непрерывность не подтверждена.
            Результаты, затронутые разрывом, не считаются нулевыми возвращениями.</p>"#,
        );
    }
    body.push_str(r#"<div class="audience-source-grid">"#);
    period(body, "current", "Текущий период", &report.current);
    period(body, "previous", "Предыдущий период", &report.previous);
    body.push_str("</div>");
    comparison(body, &report.current, &report.previous);
    body.push_str("</section>");
}

fn period(body: &mut String, key: &str, title: &str, period: &ReadingReturnsPeriod) {
    write!(
        body,
        r#"<section class="book-section-card audience-source-card" data-testid="audience-returns-{key}"
        data-coverage-complete="{}"><h2>{title}</h2><p>С {}<br>до {}.</p>
        <p>Начали наблюдение: <strong data-metric="cohort">{}</strong>.</p>"#,
        period.coverage_complete, time(&period.from_utc), time(&period.until_utc), period.enrolled
    ).expect("String");
    if !period.coverage_complete {
        body.push_str(
            r#"<p data-testid="audience-returns-partial">Измерение охватывает период не полностью.
            Числа относятся только к сохранённым наблюдениям.</p>"#,
        );
    }
    if period.enrolled == 0 {
        body.push_str(
            r#"<p data-testid="audience-returns-empty">Нет сохранённых начал наблюдения.
            Это не означает, что никто не читал.</p>"#,
        );
    }
    for metric in &period.metrics {
        metric_row(body, metric);
    }
    body.push_str("</section>");
}

fn metric_row(body: &mut String, metric: &ReadingReturnsMetric) {
    write!(
        body,
        r#"<article data-return-day="{}"><h3>На {}-й день</h3><dl>"#,
        metric.day, metric.day
    )
    .expect("String");
    for (key, label, value) in [
        ("eligible", "Можно оценить", metric.eligible),
        ("returned", "Вернулись среди оценённых", metric.returned),
        ("pending", "Ещё ждём окончания дня", metric.pending),
        (
            "unknown",
            "Неизвестно из-за разрыва измерения",
            metric.unknown,
        ),
    ] {
        // With no eligible cohort the numerator is not an observed zero rate.
        let displayed = if key == "returned" && metric.eligible == 0 {
            "—".to_owned()
        } else {
            value.to_string()
        };
        write!(
            body,
            r#"<dt>{label}</dt><dd data-metric="{key}">{displayed}</dd>"#
        )
        .expect("String");
    }
    let rate = rate(metric).map(|value| format!("{value:.1}%").replace('.', ","));
    write!(
        body,
        r#"<dt>Доля вернувшихся</dt><dd data-metric="rate">{}</dd></dl>"#,
        rate.as_deref().unwrap_or("Пока не рассчитана")
    )
    .expect("String");
    if metric.pending > 0 {
        body.push_str("<p>Ожидающие записи пока не входят в долю возвращений.</p>");
    }
    body.push_str("</article>");
}

fn rate(metric: &ReadingReturnsMetric) -> Option<f64> {
    (metric.eligible > 0).then(|| 100.0 * metric.returned as f64 / metric.eligible as f64)
}

fn comparison(body: &mut String, current: &ReadingReturnsPeriod, previous: &ReadingReturnsPeriod) {
    body.push_str(
        r#"<section class="book-section-card" data-testid="audience-returns-comparison">
        <h2>Изменение к предыдущему периоду</h2><dl>"#,
    );
    for (current_metric, previous_metric) in current.metrics.iter().zip(&previous.metrics) {
        let comparable = current.coverage_complete
            && previous.coverage_complete
            && current_metric.unknown == 0
            && previous_metric.unknown == 0;
        let change = if comparable {
            rate(current_metric)
                .zip(rate(previous_metric))
                .map(|(now, before)| {
                    format!("{:+.1} процентного пункта", now - before).replace('.', ",")
                })
        } else {
            None
        };
        write!(
            body,
            r#"<dt>На {}-й день</dt><dd data-return-day="{}">{}</dd>"#,
            current_metric.day,
            current_metric.day,
            escape_html(
                change
                    .as_deref()
                    .unwrap_or("Недостаточно завершённых сопоставимых наблюдений")
            )
        )
        .expect("String");
    }
    body.push_str("</dl><p>Сравниваются доли среди записей с законченным сроком ожидания.
        Охват и размер групп могут отличаться; это не доказательство причины изменения.</p></section>");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> ReadingReturnsReport {
        let period = ReadingReturnsPeriod {
            from_utc: "2026-08-13T00:00:00Z".into(),
            until_utc: "2026-11-11T00:00:00Z".into(),
            coverage_complete: true,
            enrolled: 10,
            metrics: [1, 7, 30].map(|day| ReadingReturnsMetric {
                day,
                eligible: 8,
                returned: 2,
                pending: 2,
                unknown: 0,
            }),
        };
        ReadingReturnsReport {
            days: 90,
            measured_at_utc: "2026-11-10T14:00:00Z".into(),
            observation_started_at_utc: Some("2026-05-01T00:00:00Z".into()),
            last_observed_at_utc: Some("2026-11-10T13:59:59Z".into()),
            collection_healthy: true,
            current: period.clone(),
            previous: period,
        }
    }

    #[test]
    fn maturity_defines_the_rate_and_pending_is_separate() {
        let mut report = sample();
        report.previous.metrics[0].returned = 1;
        let html = page(90, Some(&report)).0;
        assert!(html.contains("data-metric=\"rate\">25,0%"));
        assert!(html.contains("+12,5 процентного пункта"));
        assert!(html.contains("data-metric=\"pending\">2"));
        report.current.metrics[0].eligible = 0;
        report.current.metrics[0].returned = 0;
        let html = page(90, Some(&report)).0;
        assert!(html.contains("data-metric=\"rate\">Пока не рассчитана"));
        assert!(html.contains("data-metric=\"returned\">—"));
        assert!(!html.contains("+12,5 процентного пункта"));
    }

    #[test]
    fn missing_coverage_blocks_comparison_and_start_is_not_invented() {
        let mut report = sample();
        report.current.coverage_complete = false;
        let html = page(7, Some(&report)).0;
        assert!(html.contains("audience-returns-partial"));
        assert!(!html.contains("+0,0 процентного пункта"));
        report.observation_started_at_utc = None;
        let html = page(7, Some(&report)).0;
        assert!(html.contains("audience-returns-not-started"));
        assert!(!html.contains("data-metric=\"cohort\""));
        assert!(page(7, None).0.contains("audience-returns-unavailable"));
    }
}
