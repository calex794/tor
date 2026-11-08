//! S029: Team analytics HTML and CSV share the same filtered PostgreSQL read model.
use super::{actor, page, session_failure, team_error, TeamState};
use crate::auth_cookies::private_response;
use crate::pages::escape_html;
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, Response},
};
use serde::Deserialize;
use std::fmt::Write;
use tor_books::repositories::translation_team_analytics::{
    PgTeamAnalytics, TeamAnalyticsReport, TeamPeriod,
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReportQuery {
    period: Option<String>,
    book_id: Option<i64>,
}

async fn load(
    state: &TeamState,
    headers: &HeaderMap,
    team_id: i64,
    query: &ReportQuery,
) -> Result<TeamAnalyticsReport, Response> {
    let actor = actor(state, headers).await.map_err(session_failure)?;
    let period = TeamPeriod::parse(query.period.as_deref()).map_err(team_error)?;
    PgTeamAnalytics::new(state.accounts.pool().clone())
        .report(actor, team_id, query.book_id, period)
        .await
        .map_err(team_error)
}

fn currency(minor: i64) -> String {
    format!("{}.{:02}", minor / 100, minor.rem_euclid(100))
}

pub(super) async fn page(
    State(state): State<TeamState>,
    Path(team_id): Path<i64>,
    Query(query): Query<ReportQuery>,
    headers: HeaderMap,
) -> Response {
    let report = match load(&state, &headers, team_id, &query).await {
        Ok(report) => report,
        Err(response) => return response,
    };
    let mut body = format!(
        "<section class=\"section\"><div class=\"container\"><h1>Аналитика команды: {}</h1>\
         <p>Период UTC: {} — {} ({} дней)</p>\
         <p>Открытия глав не доказывают прочтение. Новые зарегистрированные \
         читатели учитываются при первом открытии книги, не все активные \
         уникальные за период. Сумма по книгам может учитывать человека несколько раз.</p>\
         <form method=\"get\" action=\"/me/teams/{team_id}/analytics\">\
         <label>Период <select name=\"period\">",
        escape_html(&report.team_name),
        report.date_from,
        report.date_to,
        report.period.days(),
    );
    for period in ["7d", "30d", "90d"] {
        let selected = if period == report.period.code() {
            " selected"
        } else {
            ""
        };
        let _ = write!(
            body,
            "<option value=\"{period}\"{selected}>{period}</option>"
        );
    }
    body.push_str("</select></label><label>Книга <select name=\"book_id\"><option value=\"\">Все</option>");
    let actor_id = match actor(&state, &headers).await {
        Ok(id) => id,
        Err(status) => return session_failure(status),
    };
    let view = match state.books.view(actor_id, team_id).await {
        Ok(view) => view,
        Err(error) => return team_error(error),
    };
    for book in &view.linked {
        let selected = if query.book_id == Some(book.id) {
            " selected"
        } else {
            ""
        };
        let _ = write!(
            body,
            "<option value=\"{}\"{}>{}</option>",
            book.id,
            selected,
            escape_html(&book.title)
        );
    }
    body.push_str("</select></label><button type=\"submit\">Показать</button></form>");
    let mut csv_url = format!(
        "/me/teams/{team_id}/analytics.csv?period={}",
        report.period.code()
    );
    if let Some(id) = query.book_id {
        let _ = write!(csv_url, "&book_id={id}");
    }
    let _ = write!(
        body,
        "<p><a href=\"{}\">Выгрузить этот же фильтр в CSV</a></p>",
        escape_html(&csv_url)
    );
    body.push_str(
        "<table><thead><tr><th>Книга</th><th>Открытия</th><th>Анонимные открытия</th>\
         <th>Первые открытия глав</th><th>Новые зарегистрированные читатели</th>\
         <th>Покупки</th><th>Выручка (₽)</th><th>Спасибо</th></tr></thead><tbody>",
    );
    write_row(&mut body, "Итого", &report.totals);
    for book in &report.rows {
        write_row(&mut body, &book.title, &book.numbers);
    }
    body.push_str("</tbody></table></div></section>");
    page("Аналитика команды", &body, &[])
}

fn write_row(body: &mut String, title: &str, numbers: &tor_books::repositories::translation_team_analytics::TeamNumbers) {
    let _ = write!(
        body,
        "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td>\
         <td>{}</td><td>{}</td><td>{}</td></tr>",
        escape_html(title),
        numbers.opens,
        numbers.anonymous_opens,
        numbers.first_chapter_opens,
        numbers.new_registered_readers,
        numbers.purchases,
        currency(numbers.gross_minor),
        numbers.thanks
    );
}

fn csv_text(raw: &str) -> String {
    let value = if matches!(raw.trim_start().chars().next(), Some('=' | '+' | '-' | '@' | '\t' | '\r')) {
        format!("'{raw}")
    } else {
        raw.to_owned()
    };
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn csv(report: &TeamAnalyticsReport) -> String {
    let mut content = String::from(
        "\u{feff}row_type;period_from_utc;period_to_utc;book_id;title;opens;anonymous_opens;first_chapter_opens;new_registered_readers;chapter_purchases;gross_minor;thanks\r\n",
    );
    let mut line = |kind: &str, id: Option<i64>, name: &str, values: &tor_books::repositories::translation_team_analytics::TeamNumbers| {
        let _ = write!(
            content,
            "{};{};{};{};{};{};{};{};{};{};{};{}\r\n",
            csv_text(kind),
            report.date_from,
            report.date_to,
            id.map(|x| x.to_string()).unwrap_or_default(),
            csv_text(name),
            values.opens,
            values.anonymous_opens,
            values.first_chapter_opens,
            values.new_registered_readers,
            values.purchases,
            values.gross_minor,
            values.thanks
        );
    };
    line("total", None, &report.team_name, &report.totals);
    for book in &report.rows {
        line("book", Some(book.id), &book.title, &book.numbers);
    }
    content
}

pub(super) async fn export(
    State(state): State<TeamState>,
    Path(team_id): Path<i64>,
    Query(query): Query<ReportQuery>,
    headers: HeaderMap,
) -> Response {
    let report = match load(&state, &headers, team_id, &query).await {
        Ok(report) => report,
        Err(response) => return response,
    };
    let mut response = private_response(Html(csv(&report)), &[]);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"team-analytics.csv\""),
    );
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn s029_csv_escapes_formula_and_multiline_text() {
        assert_eq!(csv_text("=SUM(A1)"), "\"'=SUM(A1)\"");
        assert_eq!(csv_text("  +evil"), "\"'  +evil\"");
        assert_eq!(csv_text("Hello \"Team\""), "\"Hello \"\"Team\"\"\"");
        assert_eq!(TeamPeriod::parse(Some("1d")).unwrap_err(), tor_books::services::translation_teams::TeamError::Invalid);
    }
}
