//! Authenticated NEwBOO book-statistics page and safe CSV export.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Html, Redirect, Response},
};
use serde::Deserialize;
use tor_books::repositories::book_members::PgBookMembers;
use tor_reading::repositories::book_stats::{BookStatsReport, PgBookStats};

#[derive(Default, Deserialize)]
pub(super) struct StatsQuery {
    after: Option<String>,
}

fn parse_after(input: Option<&str>) -> Result<Option<(i32, i64)>, StatusCode> {
    let Some(value) = input else {
        return Ok(None);
    };
    let (no, id) = value.split_once(':').ok_or(StatusCode::BAD_REQUEST)?;
    let no: i32 = no.parse().map_err(|_| StatusCode::BAD_REQUEST)?;
    let id: i64 = id.parse().map_err(|_| StatusCode::BAD_REQUEST)?;
    if no <= 0 || id <= 0 || value.len() > 48 {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(Some((no, id)))
}

async fn load(
    state: &AuthState,
    headers: &HeaderMap,
    id: i64,
    query: &StatsQuery,
) -> Result<BookStatsReport, Box<Response>> {
    let actor = match session_user(state, headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return Err(Box::new(private_response(Redirect::to("/login"), &[]))),
        Err(code) => return Err(Box::new(deny(code))),
    };
    let after = parse_after(query.after.as_deref()).map_err(|code| Box::new(deny(code)))?;
    let member = PgBookMembers::new(state.accounts.pool().clone())
        .active_role(actor, id)
        .await
        .map_err(|_| Box::new(deny(StatusCode::SERVICE_UNAVAILABLE)))?;
    if member.is_none() {
        return Err(Box::new(deny(StatusCode::NOT_FOUND)));
    }
    PgBookStats::new(state.accounts.pool().clone())
        .report(id, after)
        .await
        .map_err(|_| Box::new(deny(StatusCode::SERVICE_UNAVAILABLE)))?
        .ok_or_else(|| Box::new(deny(StatusCode::NOT_FOUND)))
}

fn money(value: i64) -> String {
    format!("{}.{:02}", value / 100, value.rem_euclid(100))
}

pub(super) async fn page(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    Query(query): Query<StatsQuery>,
    headers: HeaderMap,
) -> Response {
    let data = match load(&state, &headers, id, &query).await {
        Ok(data) => data,
        Err(response) => return *response,
    };
    let mut body = format!(
        "<section class=\"section\"><div class=\"container\"><h1>Статистика книги: {}</h1>\
         <p>Открытия глав: {} · Анонимные: {} · Авторизованные: {} · Уникальные \
         зарегистрированные читатели: {} · Повторные открытия: {}</p>\
         <p>Покупки: {} · Оборот прямых покупок: {} ₽ · Завершено глав: {} · Благодарностей: {}</p>\
         <p>Открытие страницы не доказывает прочтение. Авторские выплаты и комиссия здесь не рассчитаны.</p>\
         <p>Скачивание CSV соответствует выбранной странице глав.</p>\
         <h2>Главы</h2><table><thead><tr><th>Глава</th><th>Открытия</th>\
         <th>Уникальные зарегистрированные</th><th>Повторы</th><th>Покупки</th>\
         <th>Оборот, ₽</th><th>Завершения</th><th>Спасибо</th></tr></thead><tbody>",
        pages::escape_html(&data.title), data.opens, data.anonymous_opens,
        data.authenticated_opens, data.unique_registered, data.repeat_authenticated,
        data.purchases, money(data.purchase_minor), data.completed, data.thanks,
    );
    use std::fmt::Write;
    for row in &data.chapters {
        let _ = write!(
            body,
            "<tr><td>{}. {}</td><td>{}</td><td>{}</td>\
            <td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            row.number,
            pages::escape_html(&row.title),
            row.opens,
            row.unique_registered,
            row.repeat_authenticated,
            row.purchases,
            money(row.purchase_minor),
            row.completed,
            row.thanks
        );
    }
    body.push_str("</tbody></table>");
    let export_url = match query.after.as_deref() {
        Some(cursor) => format!("/books/{id}/stats/export.csv?after={cursor}"),
        None => format!("/books/{id}/stats/export.csv"),
    };
    let _ = write!(
        body,
        "<p><a href=\"{}\">Скачать CSV этой страницы</a></p>",
        pages::escape_html(&export_url)
    );

    if let Some(cursor) = &data.next_after {
        let _ = write!(
            body,
            "<p><a href=\"/books/{id}/stats?after={cursor}\">Следующие главы</a></p>"
        );
    }
    body.push_str(
        "<h2>Динамика за 30 дней</h2><table><thead><tr><th>День (UTC)</th>\
        <th>Открытия</th><th>Первые открытия глав</th><th>Покупки</th>\
        <th>Оборот, ₽</th><th>Спасибо</th></tr></thead><tbody>",
    );
    for row in &data.daily {
        let _ = write!(
            body,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td>\
            <td>{}</td><td>{}</td></tr>",
            row.day,
            row.opens,
            row.unique_first,
            row.purchases,
            money(row.purchase_minor),
            row.thanks
        );
    }
    body.push_str("</tbody></table><h2>Топ 5 глав по открытиям</h2><ol>");
    for chapter in &data.top_by_opens {
        let _ = write!(
            body,
            "<li>Глава {}. {} — {} открытий</li>",
            chapter.number,
            pages::escape_html(&chapter.title),
            chapter.count
        );
    }
    body.push_str("</ol><h2>Топ 5 глав по покупкам</h2><ol>");
    for chapter in &data.top_by_purchases {
        let _ = write!(
            body,
            "<li>Глава {}. {} — {} покупок</li>",
            chapter.number,
            pages::escape_html(&chapter.title),
            chapter.count
        );
    }
    body.push_str("</ol><h2>Топ 5 глав по благодарностям</h2><ol>");
    for chapter in &data.top_by_thanks {
        let _ = write!(
            body,
            "<li>Глава {}. {} — «Спасибо»: {}</li>",
            chapter.number,
            pages::escape_html(&chapter.title),
            chapter.count
        );
    }
    body.push_str("</ol></div></section>");
    private_response(
        pages::layout("Статистика книги", "Личная статистика", &body),
        &[],
    )
}

fn csv_cell(raw: &str) -> String {
    // Excel/LibreOffice formula-injection guard even after trimming spaces.
    let dangerous = matches!(
        raw.trim_start().chars().next(),
        Some('=' | '+' | '-' | '@' | '\t' | '\r')
    );
    let text = if dangerous {
        format!("'{raw}")
    } else {
        raw.to_owned()
    };
    format!("\"{}\"", text.replace('"', "\"\""))
}

pub(super) async fn export(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    Query(query): Query<StatsQuery>,
    headers: HeaderMap,
) -> Response {
    let data = match load(&state, &headers, id, &query).await {
        Ok(data) => data,
        Err(response) => return *response,
    };
    let mut csv = String::from("section,chapter_no,title,opens,anonymous,unique_registered,repeats,purchases,gross_minor,completed,thanks\n");
    use std::fmt::Write;
    let _ = writeln!(
        csv,
        "book,0,{},{},{},{},{},{},{},{},{}",
        csv_cell(&data.title),
        data.opens,
        data.anonymous_opens,
        data.unique_registered,
        data.repeat_authenticated,
        data.purchases,
        data.purchase_minor,
        data.completed,
        data.thanks
    );
    for row in &data.chapters {
        let _ = writeln!(
            csv,
            "chapter,{},{},{},{},{},{},{},{},{},{}",
            row.number,
            csv_cell(&row.title),
            row.opens,
            row.anonymous_opens,
            row.unique_registered,
            row.repeat_authenticated,
            row.purchases,
            row.purchase_minor,
            row.completed,
            row.thanks
        );
    }
    for day in &data.daily {
        let _ = writeln!(
            csv,
            "daily,0,{},{},0,{},0,{},{},0,{}",
            csv_cell(&day.day),
            day.opens,
            day.unique_first,
            day.purchases,
            day.purchase_minor,
            day.thanks
        );
    }
    let mut response = private_response(Html(csv), &[]);
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/csv; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"book-stats.csv\""),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guards_csv_formulas_and_quotes() {
        assert_eq!(csv_cell("=1+1"), "\"'=1+1\"");
        assert_eq!(csv_cell(" +SUM(A1)"), "\"' +SUM(A1)\"");
        assert_eq!(csv_cell("Тест \"книга\""), "\"Тест \"\"книга\"\"\"");
    }
    #[test]
    fn csv_cells_have_no_prefix_space() {
        let line = format!("book,0,{},1", csv_cell("=1+1"));
        assert_eq!(line, "book,0,\"'=1+1\",1");
    }

    #[test]
    fn bounds_cursor() {
        assert_eq!(parse_after(Some("1:5")).unwrap(), Some((1, 5)));
        assert!(parse_after(Some("0:4")).is_err());
        assert!(parse_after(Some("1:0")).is_err());
        assert!(parse_after(Some("a:b")).is_err());
    }
}
