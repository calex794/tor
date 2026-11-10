//! Public book metadata and anonymous aggregates; no reader histories.
mod fields;

use super::time;
use crate::pages::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_recommendations::services::audience::{
    books::{BookDiscoveryPeriod, BookDiscoveryQuery, BookDiscoveryReport, BookDiscoveryRow},
    DiscoveryHealth,
};

pub(crate) fn page(
    query: &BookDiscoveryQuery,
    report: Option<&BookDiscoveryReport>,
) -> Html<String> {
    let mut body = String::from(
        r#"<section class="section"><div class="container audience-books-width">
        <p class="eyebrow">Администратору сайта</p><h1>Какие книги замечают и читают</h1>
        <p>Выдача ссылок, наблюдаемые показы, переходы и первые чтения по книгам.
        Только общие числа, без истории отдельных читателей.</p>"#,
    );
    write!(
        body,
        r#"<p><a href="/admin/audience?days={}">Вернуться в «Аудиторию»</a></p>"#,
        query.days
    )
    .expect("String");
    fields::filters(&mut body, query, report);
    match report {
        Some(report) => report_body(&mut body, query, report),
        None => body.push_str(
            r#"<section class="book-section-card" data-testid="audience-books-unavailable">
            <h2>Отчёт временно недоступен</h2><p role="alert">Данные не получены.
            Нулевые значения не подставляются. Повторите запрос немного позже.</p></section>"#,
        ),
    }
    body.push_str(r#"<details class="book-section-card audience-books-help"><summary>Как читать этот отчёт</summary>
        <p>Показаны доступные сейчас публичные книги без возрастных ограничений.
        Закрытые, удалённые и удержанные книги исключены вместе с их числами.
        Поэтому суммы могут отличаться от общей сводки источников в «Аудитории».</p>
        <p>Жанры и теги — из нынешних карточек, а не из карточек на день события.
        Если указаны оба фильтра, книга должна подходить под оба. Группы могут пересекаться.</p>
        <p>Выданная ссылка каталога или поиска не доказывает видимый показ карточки.
        Показы рекомендаций — принятые наблюдения видимости карточек, а не доказательство внимания.</p>
        <p>Для каталога, поиска и рекомендаций день относится к выдаче карточки;
        переход, открытие и первое чтение могут случиться позднее и дополнить прежний период.
        Первые чтения с неизвестным источником относятся ко дню самого события.</p>
        <p>Переходы и открытия учитываются только из каталога, поиска и рекомендаций.
        Первые чтения включают также события с неизвестным источником.
        Первое подтверждённое начало учитывается один раз для учётной записи и книги;
        следующие главы и повторные запросы не создают второе начало.</p>
        <p>«Нет данных» означает отсутствие сохранённого измерения или неполные старые данные.
        Измеренный ноль показан как 0. «Нет сохранённых наблюдений» не означает,
        что книгу нигде не показывали или что она никому не интересна.</p>
        <p>Это не число людей и не строгая последовательность действий.
        Доля начавших чтение и возвращаемость здесь не вычисляются.</p>
        <p>Данные учитывают действующие правила согласия. Отзыв останавливает новый сбор;
        недавние сведения убираются при обновлении итогов. Обезличенные исторические
        итоги сохраняются в пределах действующего срока хранения.</p></details>
        <p><a href="/me">Вернуться в личный кабинет</a></p></div></section>"#);
    layout(
        "Книги — Аудитория",
        "Закрытый отчёт по публичным книгам.",
        &body,
    )
}

fn report_body(body: &mut String, query: &BookDiscoveryQuery, report: &BookDiscoveryReport) {
    body.push_str(r#"<section data-testid="audience-books"><h2>Сводка по выбранным книгам</h2>"#);
    write!(
        body,
        r#"<p>Подходящих книг: <strong data-testid="audience-books-count">{}</strong>.
        Суммы включают все подходящие книги, в том числе на следующих страницах.</p>
        <p>Время расчёта: {}.</p>"#,
        report.matched_books,
        time(&report.measured_at_utc)
    )
    .expect("String");
    body.push_str(
        "<p>Два соседних периода одинаковой длительности, календарные дни UTC.
        Начало включено, конец не включён. Сегодняшний день ещё не завершён.</p>",
    );
    health(body, &report.health);
    body.push_str(r#"<div class="audience-source-grid">"#);
    period(body, "current", "Текущий период", &report.current);
    period(body, "previous", "Предыдущий период", &report.previous);
    body.push_str("</div>");
    fields::comparison(body, &report.current.totals, &report.previous.totals);
    body.push_str("<h2>Книги</h2><p>По 20 книг на странице, в постоянном порядке.</p>");
    if report.rows.is_empty() {
        let message = if report.matched_books == 0 {
            "По выбранным фильтрам сейчас нет доступных публичных книг. Измените или сбросьте фильтры."
        } else {
            "После этой книги нет следующих строк. Вернитесь к началу списка."
        };
        write!(
            body,
            r#"<p data-testid="audience-books-empty">{message}</p>"#
        )
        .expect("String");
    }
    for book in &report.rows {
        book_row(body, book);
    }
    body.push_str(
        r#"<nav class="audience-books-pagination" aria-label="Страницы отчёта по книгам">"#,
    );
    if query.after_id.is_some() {
        write!(body, r#"<a class="button button--quiet" data-testid="audience-books-first" href="{}">В начало списка</a>"#,
            escape_html(&fields::url(query, None))).expect("String");
    }
    if let Some(after_id) = report.next_after_id {
        write!(
            body,
            r#"<a class="button" data-testid="audience-books-next" href="{}">Следующие книги</a>"#,
            escape_html(&fields::url(query, Some(after_id)))
        )
        .expect("String");
    }
    body.push_str("</nav></section>");
}

fn period(body: &mut String, key: &str, label: &str, period: &BookDiscoveryPeriod) {
    write!(body, r#"<section class="book-section-card audience-source-card" data-testid="audience-books-{key}" data-period="{key}">
        <h3>{label}</h3><p>С {} до {} UTC.</p>"#, escape_html(&period.from_day), escape_html(&period.until_day_exclusive)).expect("String");
    if period.includes_partial_today {
        body.push_str("<p>Включает незавершённый сегодняшний день.</p>");
    }
    fields::metrics(body, &period.totals);
    body.push_str("<h4>Состояние книг</h4><dl>");
    for (key, label, value) in [
        (
            "no-observations",
            "Нет сохранённых наблюдений",
            period.counts.no_observations,
        ),
        (
            "observed-without-starts",
            "Есть наблюдения, первых чтений нет",
            period.counts.observed_without_starts,
        ),
        (
            "with-starts",
            "Есть первые чтения",
            period.counts.with_starts,
        ),
    ] {
        write!(
            body,
            r#"<dt>{label}</dt><dd data-state="{key}">{value}</dd>"#
        )
        .expect("String");
    }
    body.push_str("</dl></section>");
}

fn book_row(body: &mut String, book: &BookDiscoveryRow) {
    write!(body, r#"<article class="book-section-card audience-book-row" data-testid="audience-book-row" data-book-id="{}">
        <h3><a href="/books/{}">{}</a></h3>"#, book.book_id, book.book_id, escape_html(&book.title)).expect("String");
    for (label, values) in [("Жанры", &book.genres), ("Теги", &book.tags)] {
        write!(
            body,
            "<p>{label}: {}.</p>",
            if values.is_empty() {
                "не указаны".to_string()
            } else {
                escape_html(&values.join(", "))
            }
        )
        .expect("String");
    }
    body.push_str(r#"<div class="audience-source-grid">"#);
    for (key, title, metrics) in [
        ("current", "Текущий период", &book.current),
        ("previous", "Предыдущий период", &book.previous),
    ] {
        let (state, label) = fields::observation(metrics.observation());
        write!(
            body,
            r#"<section class="audience-source-card" data-period="{key}" data-state="{state}">
            <h4>{title}</h4><p class="audience-book-state">{label}</p>"#
        )
        .expect("String");
        fields::metrics(body, metrics);
        body.push_str("</section>");
    }
    body.push_str("</div></article>");
}

fn health(body: &mut String, health: &DiscoveryHealth) {
    body.push_str(r#"<section class="book-section-card" data-testid="audience-books-health"><h3>Обновление данных</h3>"#);
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
