//! Current catalogue groups reuse bounded same-book observations.
use super::{book_returns, evidence, time};
use crate::pages::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_reading::services::group_book_returns::{
    GroupBookReadingReturnsKind, GroupBookReadingReturnsQuery, GroupBookReadingReturnsReport,
};

const PREFIX: &str = "audience-group-returns";

pub(crate) fn page(
    query: &GroupBookReadingReturnsQuery,
    report: Option<&GroupBookReadingReturnsReport>,
) -> Html<String> {
    let missing = match query.kind {
        GroupBookReadingReturnsKind::Genres => "Без жанра",
        GroupBookReadingReturnsKind::Tags => "Без тега",
    };
    let mut body = String::from(
        r#"<section class="section"><div class="container audience-books-width">
        <p class="eyebrow">Администратору сайта</p><h1>Возвращения к книгам группы</h1>
        <p>Подтверждённое продолжение чтения той же книги на 7-й и 30-й день.
        Одна учётная запись с двумя книгами даёт два наблюдения. Это не число людей.</p>"#,
    );
    write!(body, r#"<h2 data-testid="audience-group-returns-title">{}</h2>
        <p><a href="/admin/audience/groups?kind={}&amp;days={}">Вернуться к жанрам и тегам</a></p>
        <p><a class="button button--quiet" data-testid="audience-group-returns-books" href="{}">Открыть книги этой группы</a></p>"#,
        escape_html(query.label.as_deref().unwrap_or(missing)), query.kind.as_str(), query.days,
        escape_html(&books_url(query))).expect("String");
    filters(&mut body, query);
    match report {
        Some(report) => report_body(&mut body, report),
        None => body.push_str(
            r#"<section class="book-section-card" data-testid="audience-group-returns-unavailable">
            <h2>Отчёт временно недоступен</h2><p role="alert">Данные не получены.
            Нулевые значения не подставляются. Повторите запрос немного позже.</p></section>"#,
        ),
    }
    body.push_str(r#"<details class="book-section-card audience-books-help"><summary>Как считаются возвращения группы</summary>
        <p>В расчёт входят все нынешние публичные книги выбранного жанра или тега без
        возрастных ограничений. Состав одинаков для обоих периодов. Изменение меток переносит
        прежние числа книги в её нынешние группы; скрытая книга сразу исключается.</p>
        <p>Каждая книга учитывается один раз внутри группы, даже если метка повторяется.
        Группы могут пересекаться, поэтому складывать их числа нельзя. Общая доля — сумма
        возвращений, делённая на сумму завершённых наблюдений, а не среднее процентов книг.</p>
        <p>Наблюдение начинается с нового принятого положительного прогресса книги при действующем
        согласии и включённом измерении. Это не обязательно первое чтение за всё время.
        Старые события и сохранённые позиции не восстанавливают прошлое.</p>
        <p>Возвращение на 7-й день — новый принятый прогресс той же книги ровно в календарный день
        UTC через семь дней после начала. Так же считается 30-й день. Чтение другой книги
        этого жанра или тега не подтверждает возвращение к первой книге.</p>
        <p>Целевой день должен полностью закончиться и непрерывно измеряться.
        Сегодняшний целевой день ещё ожидается. Разрыв измерения даёт неизвестный результат,
        а не нулевое возвращение. В коротких периодах поздние возвращения ещё нельзя оценить.</p>
        <p>Главы, устройства и повторные запросы внутри одного окна не добавляют наблюдений.
        Открытие страницы и повторное сохранение того же места не подтверждают возвращение.
        Для купленной главы из защищённого хранилища нужно проверенное завершение чтения.</p>
        <p>После 30-го дня личная связь с книгой удаляется. Новое чтение может начать следующее
        окно; отзыв и новое согласие также начинают отдельное наблюдение. Одна учётная запись
        может дать несколько наблюдений за выбранный период.</p>
        <p>Используются уже существующие книжные наблюдения: личное состояние хранится максимум
        31 сутки, безличные итоги — до 180 дней. Новый журнал чтения по жанрам не создаётся.
        Здесь нет имён и списков читателей.</p></details>
        <p><a href="/me">Вернуться в личный кабинет</a></p></div></section>"#);
    layout(
        "Возвращения к книгам группы — Аудитория",
        "Закрытая сводка возвращений к книгам выбранного жанра или тега.",
        &body,
    )
}

fn filters(body: &mut String, query: &GroupBookReadingReturnsQuery) {
    write!(body, r#"<form class="author-form audience-books-filters" id="audience-group-returns-filters"
        method="get" action="/admin/audience/returns"><input type="hidden" name="kind" value="{}">"#,
        query.kind.as_str()).expect("String");
    match &query.label {
        Some(label) => write!(
            body,
            r#"<input type="hidden" name="label" value="{}">"#,
            escape_html(label)
        )
        .expect("String"),
        None => body.push_str(r#"<input type="hidden" name="without_label" value="true">"#),
    }
    body.push_str(
        r#"<div><label for="group-returns-days">Когда началось наблюдение за книгой</label>
        <select id="group-returns-days" name="days">"#,
    );
    for (value, label) in [
        (1, "1 день"),
        (7, "7 дней"),
        (30, "30 дней"),
        (90, "90 дней"),
    ] {
        let selected = if query.days == value { " selected" } else { "" };
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

fn report_body(body: &mut String, report: &GroupBookReadingReturnsReport) {
    write!(body, r#"<section data-testid="audience-group-returns" data-healthy="{}">
        <p>Доступных книг в группе: <strong data-testid="audience-group-returns-book-count">{}</strong>.
        В расчёт входят все книги группы, включая следующие страницы списка.</p>
        <p>Время расчёта: {}.</p><p>Два соседних периода одинаковой длительности по дню
        начала наблюдения за книгой. Дни указаны в UTC; начало включено, конец не включён.
        Текущий период включает сегодняшний день.</p>"#,
        report.collection_healthy, report.matched_books, time(&report.measured_at_utc)).expect("String");
    if report.matched_books == 0 {
        body.push_str(
            r#"<p class="book-section-card" data-testid="audience-group-returns-no-books">
            Сейчас в этой группе нет доступных публичных книг.</p>"#,
        );
    }
    let Some(start) = &report.observation_started_at_utc else {
        body.push_str(
            r#"<p class="book-section-card" data-testid="audience-group-returns-not-started">
            Измерение возвращений по книгам ещё не началось. Общая история возвращений
            и сохранённые места чтения не заменяют это измерение. Значения и доли пока
            не рассчитаны.</p></section>"#,
        );
        return;
    };
    write!(body, r#"<p data-testid="audience-group-returns-observation-start">Начало измерения по книгам: {}.</p>"#,
        time(start)).expect("String");
    if let Some(last) = &report.last_observed_at_utc {
        write!(body, r#"<p data-testid="audience-group-returns-last-observed">Состояние сбора проверено: {}.</p>"#,
            time(last)).expect("String");
    }
    if !report.collection_healthy {
        body.push_str(r#"<p class="book-section-card" role="status" data-testid="audience-group-returns-collection-gap">
            Сбор сейчас выключен или его непрерывность не подтверждена.
            Результаты, затронутые разрывом, не считаются нулевыми возвращениями.</p>"#);
    }
    body.push_str(r#"<div class="audience-source-grid">"#);
    book_returns::period(body, PREFIX, "current", "Текущий период", &report.current);
    book_returns::period(
        body,
        PREFIX,
        "previous",
        "Предыдущий период",
        &report.previous,
    );
    body.push_str("</div>");
    book_returns::comparison(body, PREFIX, &report.current, &report.previous);
    body.push_str("</section>");
}

fn books_url(query: &GroupBookReadingReturnsQuery) -> String {
    let key = match query.kind {
        GroupBookReadingReturnsKind::Genres => "genre",
        GroupBookReadingReturnsKind::Tags => "tag",
    };
    match &query.label {
        Some(label) => format!(
            "/admin/audience/books?days={}&{key}={}",
            query.days,
            evidence::encode(label)
        ),
        None => format!(
            "/admin/audience/books?days={}&without_{key}=true",
            query.days
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_group_is_escaped_and_links_to_its_exact_books() {
        let query = GroupBookReadingReturnsQuery::new(
            30,
            GroupBookReadingReturnsKind::Tags,
            Some("<МИР> & путь".into()),
        )
        .unwrap();
        let html = page(&query, None).0;
        assert!(html.contains("&lt;мир&gt; &amp; путь"));
        assert!(!html.contains("<мир>"));
        assert!(html.contains(r#"name="kind" value="tags""#));
        assert!(html.contains(r#"name="label" value="&lt;мир&gt; &amp; путь""#));
        assert!(html.contains(&escape_html(&books_url(&query))));
        assert!(html.contains("audience-group-returns-unavailable"));
        assert!(!html.contains("data-metric="));
        assert!(html.contains("Одна учётная запись с двумя книгами даёт два наблюдения"));
        let missing =
            GroupBookReadingReturnsQuery::new(90, GroupBookReadingReturnsKind::Genres, None)
                .unwrap();
        let html = page(&missing, None).0;
        assert!(html.contains(r#"name="without_label" value="true""#));
        assert!(html.contains("/admin/audience/books?days=90&amp;without_genre=true"));
        assert!(html.contains("Без жанра"));
    }
}
