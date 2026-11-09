//! S049: author appeal and independent administrator review for hidden Book comments.
use super::{deny, page, status, ShareState};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::{generate_session_token, session_token_digest};
use tor_social::repositories::book_comment_appeals::PgBookCommentAppeals;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AppealForm {
    csrf_token: String,
    command_key: String,
    reason: String,
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

fn store(s: &ShareState) -> PgBookCommentAppeals {
    PgBookCommentAppeals::new(s.accounts.pool().clone())
}

pub(super) async fn author_page(State(s): State<ShareState>, headers: HeaderMap) -> Response {
    let digest = match session_user(&s, &headers).await {
        Ok(digest) => digest,
        Err(error) => return deny(error),
    };
    let rows = match store(&s).author_history(&digest, 50).await {
        Ok(rows) => rows,
        Err(error) => return deny(status(error)),
    };
    let (csrf, cookies) = s.cookies.csrf_for_page(&headers);
    let mut body = String::from(
        r#"<section class="section"><div class="container">
        <h1>Обжалование скрытых комментариев</h1>
        <p>Доступны только ваши комментарии к книгам, скрытые решением администратора.
        Другой администратор рассматривает заявление. Подача не восстанавливает комментарий автоматически.</p>"#,
    );
    if rows.is_empty() {
        body.push_str("<p>Таких решений по вашим комментариям нет.</p>");
    }
    for item in rows {
        write!(
            body,
            "<article class=\"me-now-card\"><h2>Комментарий #{}</h2><p>Книга #{} · решение #{}</p>",
            item.comment_id, item.book_id, item.hide_event_id
        )
        .expect("bounded appeal history markup");
        if let Some(state) = item.appeal_state {
            let label = match state.as_str() {
                "pending" => "Ожидает рассмотрения",
                "denied" => "Скрытие оставлено в силе",
                "restored" => "Комментарий восстановлен",
                _ => "Неизвестное состояние",
            };
            write!(body, "<p>{label}</p>").expect("static label");
        } else if item.visibility == "hidden" {
            write!(
                body,
                r#"<form method="post" action="/me/comment-appeals/{}">
                <label>Обоснование апелляции (20–1000 символов)
                <textarea name="reason" minlength="20" maxlength="1000" required></textarea></label>
                <input type="hidden" name="csrf_token" value="{}">
                <input type="hidden" name="command_key" value="{}">
                <button type="submit">Подать апелляцию</button></form>"#,
                item.hide_event_id,
                super::escape_html(&csrf),
                generate_session_token()
            )
            .expect("bounded appeal form");
        } else {
            body.push_str("<p>Комментарий уже не скрыт; апелляция недоступна.</p>");
        }
        body.push_str("</article>");
    }
    body.push_str("</div></section>");
    page("Апелляции по комментариям", &body, &cookies)
}

pub(super) async fn submit(
    State(s): State<ShareState>,
    Path(hide_event_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<AppealForm>,
) -> Response {
    if !s.cookies.same_origin(&headers) || !s.cookies.valid_csrf(&headers, &form.csrf_token) {
        return deny(StatusCode::FORBIDDEN);
    }
    let digest = match session_user(&s, &headers).await {
        Ok(digest) => digest,
        Err(error) => return deny(error),
    };
    match store(&s)
        .submit(&digest, hide_event_id, &form.reason, &form.command_key)
        .await
    {
        Ok(_) => super::private_response(Redirect::to("/me/comment-appeals"), &[]),
        Err(error) => deny(status(error)),
    }
}

pub(super) async fn queue(State(s): State<ShareState>, headers: HeaderMap) -> Response {
    let digest = match session_user(&s, &headers).await {
        Ok(digest) => digest,
        Err(error) => return deny(error),
    };
    let rows = match store(&s).pending(&digest, 50).await {
        Ok(rows) => rows,
        Err(error) => return deny(status(error)),
    };
    let (csrf, cookies) = s.cookies.csrf_for_page(&headers);
    let mut body = String::from(
        r#"<section class="section"><div class="container">
        <h1>Апелляции по скрытым комментариям книг</h1>
        <p>Только администратор. Первоначальный администратор не вправе пересматривать своё решение.
        Восстановление — отдельное явное решение, не следствие подачи обращения.</p>
        <p><a href="/admin/book-reports">К очереди жалоб</a></p>"#,
    );
    if rows.is_empty() {
        body.push_str("<p>Ожидающих апелляций нет.</p>");
    }
    for item in rows {
        write!(
            body,
            r#"<article class="me-now-card"><h2>Апелляция #{}</h2>
            <p>Книга #{} · комментарий #{} · исходное решение #{}
            · автор #{} · первый администратор #{}</p>
            <p>Обоснование: {}</p>
            <form method="post" action="/admin/book-comment-appeals/{}/decision">
            <input type="hidden" name="csrf_token" value="{}">
            <label>Мотивировка решения <input name="note" maxlength="500" required></label>
            <button name="decision" value="denied" type="submit">Оставить скрытым</button>
            <button name="decision" value="restored" type="submit">Восстановить комментарий</button>
            </form></article>"#,
            item.id,
            item.book_id,
            item.comment_id,
            item.hide_event_id,
            item.author_id,
            item.original_reviewer_id,
            super::escape_html(&item.reason),
            item.id,
            super::escape_html(&csrf),
        )
        .expect("bounded admin appeal markup");
    }
    body.push_str("</div></section>");
    page("Апелляции по комментариям", &body, &cookies)
}

pub(super) async fn decide(
    State(s): State<ShareState>,
    Path(appeal_id): Path<i64>,
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
        .decide(&digest, appeal_id, &form.decision, &form.note)
        .await
    {
        Ok(()) => super::private_response(Redirect::to("/admin/book-comment-appeals"), &[]),
        Err(error) => deny(status(error)),
    }
}
