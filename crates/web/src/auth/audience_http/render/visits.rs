//! Numeric visit cohorts, with clear distinction from whole-site audience.
use crate::pages::{escape_html, layout};
use axum::response::Html;
use std::fmt::Write;
use tor_recommendations::services::visits::{FlowVisitsPeriod, FlowVisitsReport};

pub(in crate::auth::audience_http) fn page(
    days: i32,
    report: Option<&FlowVisitsReport>,
) -> Html<String> {
    let mut body = format!(
        "<section class=\"section\"><div class=\"container author-width\">\
         <p class=\"eyebrow\">Администратору сайта</p><h1>Визиты и входы</h1>\
         <p><a href=\"/admin/audience?days={days}\">Аудитория</a> · <a href=\"/admin/audience/books?days={days}\">Книги</a> · <a href=\"/admin/audience/funnel?days={days}\">Путь к чтению</a></p>\
         <p>Исторические наблюдения визитов аккаунтов, отдельно согласившихся на прежнее измерение. \
         Гости и аккаунты без согласия сюда не входят. Это не число людей и не все посещения сайта.</p>\
         <form class=\"author-form\" method=\"get\" action=\"/admin/audience/visits\">\
         <label for=\"flow-visit-days\">Длительность периода</label><select id=\"flow-visit-days\" name=\"days\">"
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
    super::retirement::notice(
        &mut body,
        "визиты, точные уникальные посетители и страницы входа и выхода",
    );
    body.push_str("<section class=\"book-section-card\"><h2>Границы измерения</h2><p>Визит объединяет наблюдаемую активность аккаунта, в том числе на разных устройствах, до перерыва в 30 минут. Последняя поверхность — последняя замеченная страница, а не подтверждённый уход. Первый источник — подсказка браузера на первом наблюдаемом запросе; он может отсутствовать или быть подменён. Вход по поисковой ссылке без доступной сессии остаётся неизвестным.</p><p>Визиты и их просмотры относятся к периоду начала визита. Уникальные аккаунты учитываются по любой сохранённой странице в выбранном периоде, один раз за весь период. Поэтому эти числа имеют разные основания. Текущий день UTC ещё не завершён. Продолжительность — интервал между первой и последней страницей, не время непрерывного чтения.</p><p>В прежней модели подробности хранились до 30 суток, обезличенные итоги — до 180 дней. При завершении этой модели оставшиеся индивидуальные записи удаляются. Без исходных страниц число аккаунтов и продолжений неизвестно; исторические агрегаты не восстанавливают эти показатели. Сохранённые итоги могут уменьшаться после удаления связанных наблюдений. Пропуски сбора не восстанавливаются.</p></section>");
    if let Some(report) = report {
        write!(
            body,
            "<p>Расчёт: {} UTC.</p>",
            escape_html(&report.measured_at_utc)
        )
        .expect("String");
        period(
            &mut body,
            "current",
            super::retirement::CURRENT,
            &report.current,
        );
        period(
            &mut body,
            "previous",
            super::retirement::PREVIOUS,
            &report.previous,
        );
    } else {
        body.push_str("<p role=\"alert\" data-testid=\"flow-visits-unavailable\">Сводка визитов временно недоступна. Нулевые значения не подставляются.</p>");
    }
    quality(&mut body);
    body.push_str("<p><a href=\"/me\">В личный кабинет</a></p></div></section>");
    layout(
        "Визиты и входы",
        "Закрытая сводка наблюдаемых визитов с согласием",
        &body,
    )
}

fn number(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "Нет измерения".to_owned())
}

fn period(body: &mut String, key: &str, title: &str, data: &FlowVisitsPeriod) {
    write!(body,"<section class=\"book-section-card\" data-testid=\"flow-visits-{key}\" data-measurement=\"legacy-consented\"><h2>{title}</h2><p>С {} до {} UTC, конец не включён.</p><dl>",escape_html(&data.from_utc),escape_html(&data.until_utc)).expect("String");
    for (metric, label, value) in [
        ("visits", "Начавшиеся наблюдаемые визиты", data.visits),
        (
            "pageviews",
            "Просмотры страниц в этих визитах",
            data.pageviews,
        ),
        (
            "unique-subjects",
            "Аккаунты с сохранённой страницей за период",
            data.unique_subjects,
        ),
        (
            "observed-seconds",
            "Сумма наблюдаемых интервалов, секунды",
            data.observed_seconds,
        ),
        (
            "unattributed-pageviews",
            "Страницы продолжений после удаления старого начала визита",
            data.unattributed_pageviews,
        ),
    ] {
        write!(
            body,
            "<dt>{label}</dt><dd data-metric=\"{metric}\">{}</dd>",
            number(value)
        )
        .expect("String");
    }
    body.push_str("</dl>");
    if data.unattributed_pageviews.is_some_and(|count| count > 0) {
        body.push_str("<p>Начало длительного визита уже удалено по сроку хранения. Его продолжение не считается новым визитом; прежний источник не восстанавливается.</p>");
    }
    if data.rows.is_empty() {
        body.push_str("<p>Нет сохранённых визитов, начавшихся в этом периоде. Это не доказывает отсутствие посетителей.</p>");
    }
    for row in &data.rows {
        write!(body,"<article class=\"book-section-card\" data-source=\"{}\" data-landing=\"{}\" data-last=\"{}\"><h3>{}</h3><p>Вход: {}. Последняя поверхность: {}.</p><dl><dt>Визиты</dt><dd>{}</dd><dt>Страницы этих визитов</dt><dd>{}</dd><dt>Наблюдаемый интервал, секунды</dt><dd>{}</dd></dl></article>",row.acquisition_hint,row.landing_surface,row.last_surface,source(row.acquisition_hint),surface(row.landing_surface),surface(row.last_surface),row.visits,row.pageviews,row.observed_seconds).expect("String");
    }
    body.push_str("</section>");
}

fn source(code: i16) -> &'static str {
    match code {
        0 => "Первый сигнал: внешний поиск",
        1 => "Первый сигнал: внешний сайт",
        2 => "Внешняя реклама",
        _ => "Источник неизвестен или прямой вход",
    }
}
fn surface(code: i16) -> &'static str {
    match code {
        0 => "главная",
        1 => "каталог",
        2 => "карточка книги",
        3 => "читалка",
        4 => "библиотека",
        _ => "неизвестно",
    }
}

fn quality(body: &mut String) {
    let snapshot = crate::recommendation_http::visits::snapshot();
    body.push_str("<section class=\"book-section-card\"><h2>Состояние текущего сборщика</h2><p>Счётчики ниже относятся только к этому процессу с его запуска. Они не измеряют весь период отчёта или другие процессы. Потеря очереди при перезапуске может быть неизвестна; отсутствие ошибок не доказывает полноту.</p>");
    if !snapshot.active {
        body.push_str("<p data-testid=\"flow-collection-off\">Сбор визитов сейчас отключён или не запущен. Сохранённые данные могут относиться к прошлому включению.</p>");
    } else {
        write!(body,"<dl><dt>Предложено очереди</dt><dd>{}</dd><dt>Подтверждено новых записей</dt><dd>{}</dd><dt>Переполнения очереди или записи без подтверждения</dt><dd>{}</dd><dt>Проверка согласия недоступна или превысила время ожидания</dt><dd>{}</dd><dt>Неуспешные попытки записи</dt><dd>{}</dd></dl>",snapshot.offered,snapshot.saved,snapshot.dropped,snapshot.scope_timeouts,snapshot.failures).expect("String");
        write!(body,"<p>Повторы или уже недопустимые наблюдения: {}. Они не считаются новыми записями; отдельно причину по этим итогам определить нельзя.</p>", snapshot.skipped).expect("String");
        if snapshot.dropped > 0 || snapshot.failures > 0 || snapshot.scope_timeouts > 0 {
            body.push_str("<p role=\"alert\">Есть пропуски или ошибки измерения. Используйте числа как сохранённые наблюдения; полная посещаемость не подтверждена.</p>");
        }
    }
    body.push_str("</section>");
}
