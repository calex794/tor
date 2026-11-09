//! S049: author appeal and independent administrator review for hidden paid V1 comments.
//! Never returns the sealed text, grants, purchased versions, or selected quotes.
use super::{deny, page, status, ShareState};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::{generate_session_token, session_token_digest};
use tor_social::repositories::paid_comment_appeals::PgPaidCommentAppeals;

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

fn store(s: &ShareState) -> PgPaidCommentAppeals {
    PgPaidCommentAppeals::new(s.accounts.pool().clone())
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
        <h1>Апелляции на скрытие комментариев платных глав</h1>
        <p>Только ваши собственные комментарии. Покупка для обжалования не требуется,
        но апелляция не предоставляет доступ к платному тексту.
        Обращение рассматривает другой администратор.</p>
        <p><a href="/me/comment-appeals">Апелляции по комментариям книг</a></p>"#,
    );
    if rows.is_empty() {
        body.push_str("<p>Нет скрытых комментариев с правом апелляции.</p>");
    }
    for item in rows {
        write!(
            body,
            "<article class=\"me-now-card\"><h2>Комментарий #{}</h2><p>Глава #{} · редакция #{} · решение #{}</p>",
            item.comment_id, item.chapter_id, item.version_id, item.hide_event_id,
        )
        .expect("bounded author paid-appeal history");
        if let Some(state) = item.appeal_state {
            let label = match state.as_str() {
                "pending" => "Ожидает рассмотрения",
                "denied" => "Скрытие оставлено в силе",
                "restored" => "Комментарий восстановлен",
                _ => "Неизвестное состояние",
            };
            write!(body, "<p>{label}</p>").expect("static appeal state");
        } else if item.visibility == "hidden" {
            write!(
                body,
                r#"<form method="post" action="/me/paid-comment-appeals/{}">
                <label>Обоснование апелляции (20–1000 символов)
                <textarea name="reason" minlength="20" maxlength="1000" required></textarea></label>
                <input type="hidden" name="csrf_token" value="{}">
                <input type="hidden" name="command_key" value="{}">
                <button type="submit">Подать апелляцию</button></form>"#,
                item.hide_event_id,
                super::escape_html(&csrf),
                generate_session_token(),
            )
            .expect("bounded author paid-appeal form");
        } else {
            body.push_str("<p>Комментарий уже не скрыт.</p>");
        }
        body.push_str("</article>");
    }
    body.push_str("</div></section>");
    page("Апелляции на платные комментарии", &body, &cookies)
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
        Ok(_) => super::private_response(Redirect::to("/me/paid-comment-appeals"), &[]),
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
        <h1>Апелляции по комментариям платных глав</h1>
        <p>Только администратор, не принимавший исходного решения.
        Восстановление влияет лишь на видимость комментария, не на платную редакцию и права чтения.</p>
        <p><a href="/admin/paid-comment-reports">К очереди жалоб</a></p>"#,
    );
    if rows.is_empty() {
        body.push_str("<p>Ожидающих апелляций нет.</p>");
    }
    for item in rows {
        write!(
            body,
            r#"<article class="me-now-card"><h2>Апелляция #{}</h2>
            <p>Глава #{} · редакция #{} · комментарий #{} · санкция #{}
            · автор #{} · исходный администратор #{}</p>
            <p>Обоснование: {}</p>
            <form method="post" action="/admin/paid-comment-appeals/{}/decision">
            <input type="hidden" name="csrf_token" value="{}">
            <label>Мотивировка решения <input name="note" maxlength="500" required></label>
            <button name="decision" value="denied" type="submit">Оставить скрытым</button>
            <button name="decision" value="restored" type="submit">Восстановить комментарий</button>
            </form></article>"#,
            item.id,
            item.chapter_id,
            item.version_id,
            item.comment_id,
            item.hide_event_id,
            item.author_id,
            item.original_reviewer_id,
            super::escape_html(&item.reason),
            item.id,
            super::escape_html(&csrf),
        )
        .expect("bounded administrator paid-appeal queue");
    }
    body.push_str("</div></section>");
    page("Апелляции по платным комментариям", &body, &cookies)
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
        Ok(()) => super::private_response(Redirect::to("/admin/paid-comment-appeals"), &[]),
        Err(error) => deny(status(error)),
    }
}
