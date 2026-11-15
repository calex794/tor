//! Public book metadata and anonymous aggregates; no reader histories.
mod fields;

use super::{evidence, time};
use crate::pages::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_recommendations::services::audience::books::{
    BookDiscoveryQuery, BookDiscoveryReport, BookDiscoveryRow,
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
        r#"<p><a href="/admin/audience?days={0}">Вернуться в «Аудиторию»</a>
        · <a href="/admin/audience/groups?days={0}&amp;kind=genres">Сравнить жанры и теги</a></p>"#,
        query.days
    )
    .expect("String");
    super::retirement::notice(
        &mut body,
        "показы, переходы и связанное с источником первое чтение",
    );
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
        <p>В прежней модели для каталога, поиска и рекомендаций день относился к выдаче карточки;
        переход, открытие и первое чтение могли случиться позднее и дополнить тот период.
        Первые чтения с неизвестным источником относятся ко дню самого события.</p>
        <p>Переходы и открытия учитываются только из каталога, поиска и рекомендаций.
        Первые чтения включают также события с неизвестным источником.
        Первое подтверждённое начало учитывается один раз для учётной записи и книги;
        следующие главы и повторные запросы не создают второе начало.</p>
        <p>«Нет данных» означает отсутствие сохранённого измерения или неполные старые данные.
        Измеренный ноль показан как 0. «Нет сохранённых наблюдений» не означает,
        что книгу нигде не показывали или что она никому не интересна.</p>
        <p>Это не число людей и не строгая последовательность действий.
        Доля начавших чтение здесь не вычисляется. Ссылка «Возвращения к книге» открывает
        отдельный отчёт о подтверждённом продолжении чтения этой книги на 7-й и 30-й день.</p>
        <p>Данные относятся к прежней выборке с согласием. Новый сбор прекращён.
        Удаление индивидуальных записей делает связанные исторические показатели неизвестными.
        Незатронутые итоги сохраняются в пределах действующего срока хранения.</p></details>
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
    evidence::health(body, "audience-books", &report.health);
    body.push_str(r#"<div class="audience-source-grid">"#);
    evidence::period(
        body,
        "audience-books",
        "current",
        super::retirement::CURRENT,
        &report.current,
    );
    evidence::period(
        body,
        "audience-books",
        "previous",
        super::retirement::PREVIOUS,
        &report.previous,
    );
    body.push_str("</div>");
    evidence::comparison(
        body,
        "audience-books",
        &report.current.totals,
        &report.previous.totals,
    );
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
        book_row(body, book, query.days);
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

fn book_row(body: &mut String, book: &BookDiscoveryRow, days: i32) {
    write!(body, r#"<article class="book-section-card audience-book-row" data-testid="audience-book-row" data-book-id="{}">
        <h3><a href="/books/{}">{}</a></h3>"#, book.book_id, book.book_id, escape_html(&book.title)).expect("String");
    write!(body, r#"<p><a data-testid="audience-book-returns-link" href="/admin/audience/returns?book_id={}&amp;days={days}">Возвращения к книге</a></p>"#, book.book_id).expect("String");
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
        ("current", super::retirement::CURRENT, &book.current),
        ("previous", super::retirement::PREVIOUS, &book.previous),
    ] {
        let (state, label) = evidence::observation(metrics.observation());
        write!(
            body,
            r#"<section class="audience-source-card" data-period="{key}" data-state="{state}">
            <h4>{title}</h4><p class="audience-book-state">{label}</p>"#
        )
        .expect("String");
        evidence::metrics(body, metrics);
        body.push_str("</section>");
    }
    body.push_str("</div></article>");
}
