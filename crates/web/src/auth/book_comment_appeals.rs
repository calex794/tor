//! S049: author appeal and independent administrator review for hidden Book comments.
use super::{deny, page, session_user, status, ShareState};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::generate_session_token;
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
        <p>Доступны только ваши комментарии к книгам, скрытые решением модератора сайта или администратора.
        Апелляцию рассматривает администратор, не принимавший исходное решение. Подача не восстанавливает комментарий автоматически.</p>
        <p><a href="/me/paid-comment-appeals">Апелляции по комментариям купленных глав</a> · <a href="/me/free-comment-appeals">Апелляции по бесплатным главам</a></p>"#,
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
            let label = super::appeal_markup::state_label(&state);
            write!(body, "<p>{label}</p>").expect("static label");
        } else if item.visibility == "hidden" {
            body.push_str(&super::appeal_markup::submit_form(
                super::appeal_markup::AppealTarget::Book,
                item.hide_event_id,
                &csrf,
                &generate_session_token(),
            ));
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
        <p>Только администратор. Сотрудник, скрывший комментарий, не вправе пересматривать своё решение.
        Восстановление — отдельное явное решение, не следствие подачи обращения.</p>
        <p><a href="/odmen/book-reports">К очереди жалоб</a></p>
        <p><a href="/odmen/paid-comment-appeals">Апелляции по купленным главам</a></p>"#,
    );
    if rows.is_empty() {
        body.push_str("<p>Ожидающих апелляций нет.</p>");
    }
    for item in rows {
        write!(
            body,
            r#"<article class="me-now-card"><h2>Апелляция #{}</h2>
            <p>Книга #{} · комментарий #{} · исходное решение #{}
            · автор #{} · скрыл сотрудник #{}</p>
            <p>Обоснование: {}</p>
            {}</article>"#,
            item.id,
            item.book_id,
            item.comment_id,
            item.hide_event_id,
            item.author_id,
            item.original_reviewer_id,
            super::escape_html(&item.reason),
            super::appeal_markup::decision_form(
                super::appeal_markup::AppealTarget::Book,
                item.id,
                &csrf,
            ),
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
        Ok(()) => super::private_response(Redirect::to("/odmen/book-comment-appeals"), &[]),
        Err(error) => deny(status(error)),
    }
}
