//! S049: private administrator inbox for reports about shared books.
//! A site moderator can review Book registration, not chapter or share reports.
use super::{deny, page, status, ShareState};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::session_token_digest;
use tor_social::repositories::book_share_reports::PgBookShareReports;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ReviewForm {
    csrf_token: String,
    decision: String,
    note: String,
}

fn session_digest(s: &ShareState, headers: &HeaderMap) -> Result<[u8; 32], StatusCode> {
    let token = s.cookies.session_token(headers).ok_or(StatusCode::UNAUTHORIZED)?;
    Ok(session_token_digest(&token))
}

pub(super) async fn queue(State(s): State<ShareState>, headers: HeaderMap) -> Response {
    let digest = match session_digest(&s, &headers) {
        Ok(digest) => digest,
        Err(error) => return deny(error),
    };
    let reports = match PgBookShareReports::new(s.accounts.pool().clone())
        .pending(&digest, 50)
        .await
    {
        Ok(reports) => reports,
        Err(error) => return deny(status(error)),
    };
    let (csrf, cookies) = s.cookies.csrf_for_page(&headers);
    let mut body = String::from(
        r#"<section class="section"><div class="container">
        <h1>Жалобы на рекомендации книг</h1>
        <p>Только администратор сайта. Решение по сообщению не блокирует книгу,
        автора, покупку или приватные главы автоматически.</p>"#,
    );
    if reports.is_empty() {
        body.push_str("<p>Ожидающих жалоб нет.</p>");
    }
    for item in reports {
        write!(
            body,
            r#"<article class="me-now-card">
            <h2>Жалоба #{}</h2><p>Книга #{} · отправитель #{} · получатель #{}</p>
            <p>Причина: {}</p><p>Сообщение: {}</p><p>Дополнение: {}</p>
            <form method="post" action="/admin/recommendation-reports/{}/decision">
            <input type="hidden" name="csrf_token" value="{}">
            <label>Комментарий решения<input name="note" maxlength="500" required></label>
            <button type="submit" name="decision" value="reviewed">Признать обоснованной</button>
            <button type="submit" name="decision" value="dismissed">Отклонить жалобу</button>
            </form></article>"#,
            item.share_id,
            item.book_id,
            item.sender_id,
            item.recipient_id,
            super::escape_html(&item.reason),
            super::escape_html(&item.message),
            super::escape_html(&item.details),
            item.share_id,
            super::escape_html(&csrf),
        )
        .expect("bounded S049 report markup");
    }
    body.push_str("</div></section>");
    page("Проверка жалоб", &body, &cookies)
}

pub(super) async fn decide(
    State(s): State<ShareState>,
    Path(share_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<ReviewForm>,
) -> Response {
    if !s.cookies.same_origin(&headers)
        || !s.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let digest = match session_digest(&s, &headers) {
        Ok(digest) => digest,
        Err(error) => return deny(error),
    };
    match PgBookShareReports::new(s.accounts.pool().clone())
        .decide(&digest, share_id, &form.decision, &form.note)
        .await
    {
        Ok(()) => deny_redirect(),
        Err(error) => deny(status(error)),
    }
}

fn deny_redirect() -> Response {
    super::private_response(Redirect::to("/admin/recommendation-reports"), &[])
}
