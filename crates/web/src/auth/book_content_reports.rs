//! S049: complaint about a published Book, separate from shared recommendations.
use super::{deny, page, status, ShareState};
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::{generate_session_token, session_token_digest};
use tor_social::repositories::book_content_reports::PgBookContentReports;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Submitted {
    sent: Option<String>,
    comment_id: Option<i64>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReportForm {
    csrf_token: String,
    command_key: String,
    comment_id: Option<i64>,
    reason: String,
    details: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DecisionForm {
    csrf_token: String,
    decision: String,
    note: String,
}

async fn session_user(s: &ShareState, headers: &HeaderMap) -> Result<[u8; 32], StatusCode> {
    let token = s
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let digest = session_token_digest(&token);
    s.accounts
        .active_session_user(&digest)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(digest)
}

fn store(s: &ShareState) -> PgBookContentReports {
    PgBookContentReports::new(s.accounts.pool().clone())
}

pub(super) async fn form(
    State(s): State<ShareState>,
    Path(id): Path<i64>,
    Query(query): Query<Submitted>,
    headers: HeaderMap,
) -> Response {
    let digest = match session_user(&s, &headers).await {
        Ok(digest) => digest,
        Err(error) => return deny(error),
    };
    let title = match store(&s).reportable_title(&digest, id).await {
        Ok(title) => title,
        Err(error) => return deny(status(error)),
    };
    if let Some(comment_id) = query.comment_id {
        match store(&s).reportable_comment(id, comment_id).await {
            Ok(true) => {}
            Ok(false) => return deny(StatusCode::NOT_FOUND),
            Err(error) => return deny(status(error)),
        }
    }
    let (csrf, cookies) = s.cookies.csrf_for_page(&headers);
    let sent = if query.sent.as_deref() == Some("1") {
        "<p role=\"status\">Жалоба зарегистрирована.</p>"
    } else {
        ""
    };
    let body = format!(
        r#"<section class="section"><div class="container">
        <h1>Пожаловаться на книгу или комментарий</h1><p>{}</p>
        <p>Обращение рассматривает администратор. Отправка не скрывает книгу автоматически.</p>
        {sent}
        <form method="post" action="/books/{id}/report">
        <label>Причина <select name="reason" required>
        <option value="spam">Спам</option>
        <option value="abuse">Оскорбления или угрозы</option>
        <option value="misleading">Недостоверное описание</option>
        <option value="other">Другое</option>
        </select></label>
        <label>Подробности <textarea name="details" maxlength="500"></textarea></label>
        {}
        <input type="hidden" name="command_key" value="{}">
        <input type="hidden" name="csrf_token" value="{}">
        <button type="submit">Отправить жалобу</button></form>
        <p><a href="/books/{id}">Вернуться к книге</a></p>
        </div></section>"#,
        super::escape_html(&title),
        query
            .comment_id
            .map_or_else(String::new, |comment_id| format!(
                r#"<input type="hidden" name="comment_id" value="{comment_id}">"#
            )),
        generate_session_token(),
        super::escape_html(&csrf),
    );
    page("Жалоба на книгу", &body, &cookies)
}

pub(super) async fn submit(
    State(s): State<ShareState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<ReportForm>,
) -> Response {
    if !s.cookies.same_origin(&headers) || !s.cookies.valid_csrf(&headers, &form.csrf_token) {
        return deny(StatusCode::FORBIDDEN);
    }
    let digest = match session_user(&s, &headers).await {
        Ok(digest) => digest,
        Err(error) => return deny(error),
    };
    let result = if let Some(comment_id) = form.comment_id {
        store(&s)
            .submit_comment(
                &digest,
                id,
                comment_id,
                &form.reason,
                &form.details,
                &form.command_key,
            )
            .await
    } else {
        store(&s)
            .submit(&digest, id, &form.reason, &form.details, &form.command_key)
            .await
    };
    match result {
        Ok(_) => super::private_response(Redirect::to(&format!("/books/{id}/report?sent=1")), &[]),
        Err(error) => deny(status(error)),
    }
}

pub(super) async fn queue(State(s): State<ShareState>, headers: HeaderMap) -> Response {
    let digest = match session_user(&s, &headers).await {
        Ok(digest) => digest,
        Err(error) => return deny(error),
    };
    let reports = match store(&s).pending(&digest, 50).await {
        Ok(reports) => reports,
        Err(error) => return deny(status(error)),
    };
    let (csrf, cookies) = s.cookies.csrf_for_page(&headers);
    let mut body = String::from(
        r#"<section class="section"><div class="container">
        <h1>Жалобы на книги</h1>
        <p>Только администратор. Решение по жалобе не меняет публикацию книги автоматически.</p>
        <p><a href="/admin/recommendation-reports">Жалобы на рекомендации</a></p>"#,
    );
    if reports.is_empty() {
        body.push_str("<p>Ожидающих жалоб нет.</p>");
    }
    for item in reports {
        write!(
            body,
            r#"<article class="me-now-card">
            <h2>Жалоба #{}</h2>
            <p>Книга #{} · {} · заявитель #{}</p><p>Причина: {}</p><p>Комментарий: {}</p>
            <form method="post" action="/admin/book-reports/{}/decision">
            <input type="hidden" name="csrf_token" value="{}">
            <label>Комментарий решения
            <input name="note" maxlength="500" required></label>
            <button type="submit" name="decision" value="reviewed">Признать обоснованной</button>
            <button type="submit" name="decision" value="dismissed">Отклонить</button>
            </form></article>"#,
            item.id,
            item.book_id,
            item.comment_id.map_or_else(
                || "Жалоба на книгу".to_owned(),
                |id| format!("Комментарий #{id}")
            ),
            item.reporter_id,
            super::escape_html(&item.reason),
            super::escape_html(&item.details),
            item.id,
            super::escape_html(&csrf),
        )
        .expect("bounded S049 Book report markup");
    }
    body.push_str("</div></section>");
    page("Рассмотрение жалоб на книги", &body, &cookies)
}

pub(super) async fn decide(
    State(s): State<ShareState>,
    Path(report_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<DecisionForm>,
) -> Response {
    if !s.cookies.same_origin(&headers) || !s.cookies.valid_csrf(&headers, &form.csrf_token) {
        return deny(StatusCode::FORBIDDEN);
    }
    let digest = match session_user(&s, &headers).await {
        Ok(digest) => digest,
        Err(error) => return deny(error),
    };
    match store(&s)
        .decide(&digest, report_id, &form.decision, &form.note)
        .await
    {
        Ok(()) => super::private_response(Redirect::to("/admin/book-reports"), &[]),
        Err(error) => deny(status(error)),
    }
}
