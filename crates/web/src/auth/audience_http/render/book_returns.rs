//! Book observations are bounded episodes, not lifetime unique readers.
use super::{
    returns::{metric_row, rate},
    time,
};
use crate::pages::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_reading::services::book_returns::{BookReadingReturnsPeriod, BookReadingReturnsReport};

pub(crate) fn page(
    book_id: i64,
    days: i32,
    report: Option<&BookReadingReturnsReport>,
) -> Html<String> {
    let mut body = String::from(
        r#"<section class="section"><div class="container audience-books-width">
        <p class="eyebrow">Администратору сайта</p><h1>Возвращаются ли к этой книге</h1>
        <p>Подтверждённое продолжение чтения выбранной книги на 7-й и 30-й день.
        Считаются наблюдения, а не уникальные люди за всё время.</p>"#,
    );
    if let Some(report) = report {
        write!(
            body,
            r#"<h2 data-testid="audience-book-returns-title">{}</h2>"#,
            escape_html(&report.book_title)
        )
        .expect("String");
    }
    write!(
        body,
        r#"<p><a href="/admin/audience/books?days={days}">Вернуться к книгам</a>
        · <a href="/admin/audience/returns?days={days}">Общие возвращения к чтению</a></p>"#
    )
    .expect("String");
    filters(&mut body, book_id, days);
    match report {
        Some(report) => report_body(&mut body, report),
        None => body.push_str(
            r#"<section class="book-section-card" data-testid="audience-book-returns-unavailable">
            <h2>Отчёт временно недоступен</h2><p role="alert">Данные не получены.
            Нулевые значения не подставляются. Повторите запрос немного позже.</p></section>"#,
        ),
    }
    body.push_str(
        r#"<details class="book-section-card audience-books-help"><summary>Как считается возвращение к книге</summary>
        <p>Наблюдение начинается с нового принятого положительного прогресса этой книги
        после включения измерения и при действующем согласии. Это не обязательно первое
        чтение книги за всё время. Старые позиции и события не восстанавливают прошлое.</p>
        <p>Возвращение на 7-й день — новый принятый прогресс этой же книги ровно в календарный
        день UTC через семь дней после начала наблюдения. Так же считается 30-й день.
        Чтение другой книги и любое чтение в течение недели или месяца не заменяют этот факт.</p>
        <p>В долю входят только наблюдения, для которых целевой день полностью закончился
        и непрерывно измерялся. Если целевой день — сегодня, результат ещё ожидается.
        В коротком выбранном периоде большинство поздних возвращений пока нельзя оценить.</p>
        <p>Несколько глав, устройства и повторные запросы в одном окне не создают новых
        наблюдений. Открытие страницы, повторное чтение того же места и отклонённое
        сохранение прогресса не подтверждают возвращение. Для купленной главы из защищённого
        хранилища требуется проверенное завершение; промежуточной позиции недостаточно.</p>
        <p>Окно заканчивается после 30-го дня. Личная связь с книгой удаляется, и следующее
        подтверждённое чтение может начать новое окно. Поэтому одна учётная запись может
        дать несколько наблюдений за выбранные 90 дней. Отзыв и новое согласие также
        начинают отдельное наблюдение; это не доказательство появления нового человека.</p>
        <p>Псевдоним согласия, книга, день начала и два признака возвращения хранятся
        максимум 31 сутки. Затем остаются только безличные итоги до 180 дней.
        Отзыв согласия удаляет личное состояние; прежние безличные итоги могут сохраняться.</p>
        <p>В отчёте доступны только нынешние публичные книги без возрастных ограничений.
        Здесь нет имён, списков читателей и их личной истории. Неизвестный результат
        из-за разрыва сбора не считается нулевым возвращением.</p></details>
        <p><a href="/me">Вернуться в личный кабинет</a></p></div></section>"#,
    );
    layout(
        "Возвращения к книге — Аудитория",
        "Закрытая сводка возвращений к выбранной книге.",
        &body,
    )
}

fn filters(body: &mut String, book_id: i64, days: i32) {
    write!(body, r#"<form class="author-form audience-books-filters" id="audience-book-returns-filters"
        method="get" action="/admin/audience/returns"><input type="hidden" name="book_id" value="{book_id}">
        <div><label for="book-returns-days">Когда началось наблюдение за книгой</label>
        <select id="book-returns-days" name="days">"#).expect("String");
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

fn report_body(body: &mut String, report: &BookReadingReturnsReport) {
    write!(
        body,
        r#"<section data-testid="audience-book-returns" data-healthy="{}">
        <p>Время расчёта: {}.</p><p>Два соседних периода одинаковой длительности по дню
        начала наблюдения. Дни указаны в UTC; начало включено, конец не включён.
        Текущий период включает сегодняшний день.</p>"#,
        report.collection_healthy,
        time(&report.measured_at_utc)
    )
    .expect("String");
    let Some(start) = report.observation_started_at_utc.as_ref() else {
        body.push_str(
            r#"<p class="book-section-card" data-testid="audience-book-returns-not-started">
            Измерение возвращений по книгам ещё не началось. Общая история возвращений
            и сохранённые места чтения не заменяют это измерение. Значения и доли пока
            не рассчитаны.</p></section>"#,
        );
        return;
    };
    write!(body, r#"<p data-testid="audience-book-returns-observation-start">Начало измерения по книгам: {}.</p>"#,
        time(start)).expect("String");
    if let Some(last) = &report.last_observed_at_utc {
        write!(body, r#"<p data-testid="audience-book-returns-last-observed">Состояние сбора проверено: {}.</p>"#,
            time(last)).expect("String");
    }
    if !report.collection_healthy {
        body.push_str(r#"<p class="book-section-card" role="status" data-testid="audience-book-returns-collection-gap">
            Сбор сейчас выключен или его непрерывность не подтверждена.
            Результаты, затронутые разрывом, не считаются нулевыми возвращениями.</p>"#);
    }
    body.push_str(r#"<div class="audience-source-grid">"#);
    period(body, "current", "Текущий период", &report.current);
    period(body, "previous", "Предыдущий период", &report.previous);
    body.push_str("</div>");
    comparison(body, &report.current, &report.previous);
    body.push_str("</section>");
}

fn period(body: &mut String, key: &str, title: &str, period: &BookReadingReturnsPeriod) {
    write!(body, r#"<section class="book-section-card audience-source-card" data-testid="audience-book-returns-{key}"
        data-coverage-complete="{}"><h2>{title}</h2><p>С {}<br>до {}.</p>
        <p>Начатых наблюдений: <strong data-metric="cohort">{}</strong>.</p>"#,
        period.coverage_complete, time(&period.from_utc), time(&period.until_utc), period.enrolled).expect("String");
    if !period.coverage_complete {
        body.push_str(r#"<p data-testid="audience-book-returns-partial">Измерение охватывает период не полностью.
            Числа относятся только к сохранённым наблюдениям.</p>"#);
    }
    if period.enrolled == 0 {
        body.push_str(
            r#"<p data-testid="audience-book-returns-empty">Нет сохранённых начал наблюдения.
            Это не означает, что книгу никто не читал.</p>"#,
        );
    }
    for metric in &period.metrics {
        metric_row(body, metric);
    }
    body.push_str("</section>");
}

fn comparison(
    body: &mut String,
    current: &BookReadingReturnsPeriod,
    previous: &BookReadingReturnsPeriod,
) {
    body.push_str(
        r#"<section class="book-section-card" data-testid="audience-book-returns-comparison">
        <h2>Изменение к предыдущему периоду</h2><dl>"#,
    );
    for (now, before) in current.metrics.iter().zip(&previous.metrics) {
        let comparable = current.coverage_complete
            && previous.coverage_complete
            && now.unknown == 0
            && before.unknown == 0;
        let change = if comparable {
            rate(now)
                .zip(rate(before))
                .map(|(a, b)| format!("{:+.1} процентного пункта", a - b).replace('.', ","))
        } else {
            None
        };
        write!(
            body,
            r#"<dt>На {}-й день</dt><dd data-return-day="{}">{}</dd>"#,
            now.day,
            now.day,
            escape_html(
                change
                    .as_deref()
                    .unwrap_or("Недостаточно завершённых сопоставимых наблюдений")
            )
        )
        .expect("String");
    }
    body.push_str(
        "</dl><p>Сравниваются доли среди завершённых наблюдений. Охват и состав групп
        могут отличаться; это не доказательство причины изменения.</p></section>",
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use tor_reading::services::returns::ReadingReturnsMetric;

    fn sample() -> BookReadingReturnsReport {
        let period = BookReadingReturnsPeriod {
            from_utc: "2026-08-13T00:00:00Z".into(),
            until_utc: "2026-11-11T00:00:00Z".into(),
            coverage_complete: true,
            enrolled: 10,
            metrics: [7, 30].map(|day| ReadingReturnsMetric {
                day,
                eligible: 8,
                returned: 2,
                pending: 2,
                unknown: 0,
            }),
        };
        BookReadingReturnsReport {
            book_id: 19,
            book_title: "<script> & книга".into(),
            days: 90,
            measured_at_utc: "2026-11-10T14:00:00Z".into(),
            observation_started_at_utc: Some("2026-05-01T00:00:00Z".into()),
            last_observed_at_utc: None,
            collection_healthy: true,
            current: period.clone(),
            previous: period,
        }
    }

    #[test]
    fn selected_book_is_escaped_and_preserved_without_inventing_unique_readers() {
        let html = page(19, 90, Some(&sample())).0;
        assert!(html.contains("&lt;script&gt; &amp; книга"));
        assert!(!html.contains("<script> & книга"));
        assert!(html.contains(r#"name="book_id" value="19""#));
        assert!(html.contains("несколько наблюдений за выбранные 90 дней"));
        assert!(!html.contains(r#"data-return-day="1""#));
    }

    #[test]
    fn pending_unknown_and_unstarted_are_not_zero_rates() {
        let mut report = sample();
        report.current.metrics[0].eligible = 0;
        report.current.metrics[0].returned = 0;
        report.current.metrics[0].unknown = 2;
        let html = page(19, 90, Some(&report)).0;
        assert!(html.contains(r#"data-metric="rate">Пока не рассчитана"#));
        assert!(html.contains(r#"data-metric="returned">—"#));
        assert!(html.contains(r#"data-metric="pending">2"#));
        assert!(html.contains("Недостаточно завершённых сопоставимых наблюдений"));
        report.observation_started_at_utc = None;
        let html = page(19, 90, Some(&report)).0;
        assert!(html.contains("audience-book-returns-not-started"));
        assert!(!html.contains("data-metric="));
        assert!(page(19, 90, None)
            .0
            .contains("audience-book-returns-unavailable"));
    }
}
