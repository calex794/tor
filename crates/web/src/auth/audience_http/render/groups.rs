//! Current catalogue labels over the same anonymous evidence as the book report.
use super::{evidence, time};
use crate::pages::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_recommendations::services::audience::groups::{
    GroupDiscoveryKind, GroupDiscoveryQuery, GroupDiscoveryReport, GroupDiscoveryRow,
};

pub(crate) fn page(
    query: &GroupDiscoveryQuery,
    report: Option<&GroupDiscoveryReport>,
) -> Html<String> {
    let mut body = String::from(
        r#"<section class="section"><div class="container audience-books-width">
        <p class="eyebrow">Администратору сайта</p><h1>Жанры и теги</h1>
        <p>Сравните направления каталога и откройте книги выбранной группы.
        Только общие числа, без истории отдельных читателей.</p>"#,
    );
    write!(
        body,
        r#"<p><a href="/admin/audience?days={0}">Вернуться в «Аудиторию»</a>
        · <a href="/admin/audience/books?days={0}">Все книги</a></p>"#,
        query.days
    )
    .expect("String");
    filters(&mut body, query);
    match report {
        Some(report) => report_body(&mut body, query, report),
        None => body.push_str(
            r#"<section class="book-section-card" data-testid="audience-groups-unavailable">
            <h2>Отчёт временно недоступен</h2><p role="alert">Данные не получены.
            Нулевые значения не подставляются. Повторите запрос немного позже.</p></section>"#,
        ),
    }
    body.push_str(r#"<details class="book-section-card audience-books-help"><summary>Как читать этот отчёт</summary>
        <p>Показаны жанры и теги нынешних доступных публичных книг без возрастных ограничений.
        Закрытые, удалённые и удержанные книги исключены вместе с их числами.
        Изменение карточки переносит её прежние числа в нынешние группы.</p>
        <p>Одна книга учитывается один раз внутри каждой своей группы. Группы пересекаются:
        их числа нельзя складывать. В общей сводке каждая книга учитывается один раз.
        Это не число людей и не доля начавших чтение.</p>
        <p>Выданная ссылка каталога или поиска не доказывает видимый показ карточки.
        Показы рекомендаций — сохранённые наблюдения видимости, а не доказательство внимания.</p>
        <p>Переходы и открытия относятся к каталогу, поиску и рекомендациям.
        Первые чтения включают также неизвестный источник. Первое начало учитывается один раз
        для учётной записи и книги; повторные запросы и следующие главы не добавляют начало.</p>
        <p>Для внутренних источников день относится к выдаче карточки: последующие действия могут
        дополнить прежний период. Начала с неизвестным источником относятся ко дню самого события.</p>
        <p>«Нет данных» означает отсутствующее или неполное измерение. Измеренный ноль показан как 0.
        «Нет сохранённых наблюдений» не означает, что книги нигде не показывали или что они неинтересны.</p>
        <p>Сбор учитывает действующее согласие. Отзыв останавливает новые события;
        недавние сведения убираются при пересчёте. Обезличенные прежние итоги сохраняются
        в пределах действующего срока хранения. Возвращаемость здесь не измеряется.</p></details>
        <p><a href="/me">Вернуться в личный кабинет</a></p></div></section>"#);
    layout(
        "Жанры и теги — Аудитория",
        "Закрытое сравнение направлений каталога.",
        &body,
    )
}

fn kind_key(kind: GroupDiscoveryKind) -> &'static str {
    match kind {
        GroupDiscoveryKind::Genres => "genres",
        GroupDiscoveryKind::Tags => "tags",
    }
}

fn filters(body: &mut String, query: &GroupDiscoveryQuery) {
    body.push_str(
        r#"<form class="author-form audience-books-filters audience-groups-filters"
        id="audience-groups-filters" method="get" action="/admin/audience/groups">
        <div><label for="groups-kind">Сравнить</label><select id="groups-kind" name="kind">"#,
    );
    for (kind, label) in [
        (GroupDiscoveryKind::Genres, "Жанры"),
        (GroupDiscoveryKind::Tags, "Теги"),
    ] {
        let selected = if kind == query.kind { " selected" } else { "" };
        write!(
            body,
            r#"<option value="{}"{selected}>{label}</option>"#,
            kind_key(kind)
        )
        .expect("String");
    }
    body.push_str(
        r#"</select></div><div><label for="groups-days">Длительность периода</label>
        <select id="groups-days" name="days">"#,
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

fn report_body(body: &mut String, query: &GroupDiscoveryQuery, report: &GroupDiscoveryReport) {
    write!(
        body,
        r#"<section data-testid="audience-groups" data-kind="{}"><h2>Общая сводка</h2>
        <p>Доступных книг: <strong data-testid="audience-groups-book-count">{}</strong>.
        Групп: <strong data-testid="audience-groups-count">{}</strong>.</p>
        <p>В этой сводке каждая книга учтена один раз. Числа включают все страницы.
        Группы ниже могут пересекаться — их суммы нельзя складывать.</p>
        <p>Время расчёта: {}.</p>
        <p>Два соседних периода одинаковой длительности, календарные дни UTC.
        Начало включено, конец не включён. Сегодняшний день ещё не завершён.</p>"#,
        kind_key(query.kind),
        report.matched_books,
        report.matched_groups,
        time(&report.measured_at_utc)
    )
    .expect("String");
    evidence::health(body, "audience-groups", &report.health);
    body.push_str(r#"<div class="audience-source-grid">"#);
    evidence::period(
        body,
        "audience-groups",
        "current",
        "Текущий период",
        &report.current,
    );
    evidence::period(
        body,
        "audience-groups",
        "previous",
        "Предыдущий период",
        &report.previous,
    );
    body.push_str("</div>");
    evidence::comparison(
        body,
        "audience-groups",
        &report.current.totals,
        &report.previous.totals,
    );
    let label = if query.kind == GroupDiscoveryKind::Genres {
        "Жанры"
    } else {
        "Теги"
    };
    write!(body, "<h2>{label}</h2><p>По 20 групп на странице. Сначала книги без метки, затем названия в постоянном порядке.</p>").expect("String");
    if report.rows.is_empty() {
        let message = if report.matched_groups == 0 {
            "Сейчас нет доступных публичных книг для сравнения."
        } else {
            "После этой группы нет следующих строк. Вернитесь к началу списка."
        };
        write!(
            body,
            r#"<p data-testid="audience-groups-empty">{message}</p>"#
        )
        .expect("String");
    }
    for group in &report.rows {
        group_row(body, query, group);
    }
    body.push_str(
        r#"<nav class="audience-books-pagination" aria-label="Страницы сравнения жанров и тегов">"#,
    );
    if query.after.is_some() {
        write!(body, r#"<a class="button button--quiet" data-testid="audience-groups-first" href="{}">В начало списка</a>"#,
            escape_html(&url(query, None))).expect("String");
    }
    if let Some(after) = &report.next_after {
        write!(body, r#"<a class="button" data-testid="audience-groups-next" href="{}">Следующие группы</a>"#,
            escape_html(&url(query, Some(after)))).expect("String");
    }
    body.push_str("</nav></section>");
}

fn group_row(body: &mut String, query: &GroupDiscoveryQuery, group: &GroupDiscoveryRow) {
    let missing = match query.kind {
        GroupDiscoveryKind::Genres => "Без жанра",
        GroupDiscoveryKind::Tags => "Без тега",
    };
    write!(body, r#"<article class="book-section-card audience-book-row" data-testid="audience-group-row"
        data-label="{}" data-missing="{}" data-kind="{}">
        <h3>{}</h3><p>Книг: <strong data-testid="audience-group-book-count">{}</strong>.</p>
        <p><a class="button button--quiet" data-testid="audience-group-books" href="{}">Открыть книги этой группы</a></p>
        <div class="audience-source-grid">"#,
        escape_html(group.label.as_deref().unwrap_or("")), group.label.is_none(), kind_key(query.kind),
        escape_html(group.label.as_deref().unwrap_or(missing)), group.matched_books,
        escape_html(&books_url(query, group.label.as_deref()))).expect("String");
    for (key, title, metrics, counts) in [
        (
            "current",
            "Текущий период",
            &group.current,
            &group.current_counts,
        ),
        (
            "previous",
            "Предыдущий период",
            &group.previous,
            &group.previous_counts,
        ),
    ] {
        write!(
            body,
            r#"<section class="audience-source-card" data-period="{key}"><h4>{title}</h4>"#
        )
        .expect("String");
        evidence::metrics(body, metrics);
        evidence::counts(body, counts);
        body.push_str("</section>");
    }
    body.push_str("</div>");
    evidence::comparison(body, "audience-group", &group.current, &group.previous);
    body.push_str("</article>");
}

fn url(query: &GroupDiscoveryQuery, after: Option<&str>) -> String {
    let mut url = format!(
        "/admin/audience/groups?days={}&kind={}",
        query.days,
        kind_key(query.kind)
    );
    if let Some(after) = after {
        write!(url, "&after={}", evidence::encode(after)).expect("String");
    }
    url
}

fn books_url(query: &GroupDiscoveryQuery, label: Option<&str>) -> String {
    let key = match query.kind {
        GroupDiscoveryKind::Genres => "genre",
        GroupDiscoveryKind::Tags => "tag",
    };
    match label {
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
