//! S049: author appeal and independent administrator review of hidden free-edition comments.
//! Never returns chapter text, selected quotes, or read grants.
use super::appeal_markup::{author_action, AppealForm, AppealTarget, DecisionForm};
use super::{deny, page, session_user, status, ShareState};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use std::fmt::Write;
use tor_social::repositories::free_comment_appeals::PgFreeCommentAppeals;

fn store(s: &ShareState) -> PgFreeCommentAppeals {
    PgFreeCommentAppeals::new(s.accounts.pool().clone())
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
        <h1>Апелляции на скрытие комментариев бесплатных глав</h1>
        <p>Только ваши собственные скрытые комментарии. Даже если глава больше
        не опубликована, апелляцию можно подать без доступа к её тексту.
        Обращение рассматривает другой администратор.</p>
        <p><a href="/me/comment-appeals">Апелляции по комментариям книг</a> · <a href="/me/paid-comment-appeals">Апелляции по платным главам</a></p>"#,
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
        .expect("bounded author free-appeal history");
        body.push_str(&author_action(
            AppealTarget::Free,
            item.hide_event_id,
            item.appeal_state.as_deref(),
            &item.visibility,
            &csrf,
        ));
        body.push_str("</article>");
    }
    body.push_str("</div></section>");
    page("Апелляции на бесплатные комментарии", &body, &cookies)
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
        Ok(_) => super::private_response(Redirect::to("/me/free-comment-appeals"), &[]),
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
        <h1>Апелляции по комментариям бесплатных глав</h1>
        <p>Только администратор, не принимавший исходного решения.
        Восстановление влияет только на видимость комментария, не на редакцию главы и права чтения.</p>
        <p><a href="/odmen/free-comment-reports">К очереди жалоб</a></p>"#,
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
            {}</article>"#,
            item.id,
            item.chapter_id,
            item.version_id,
            item.comment_id,
            item.hide_event_id,
            item.author_id,
            item.original_reviewer_id,
            super::escape_html(&item.reason),
            super::appeal_markup::decision_form(
                super::appeal_markup::AppealTarget::Free,
                item.id,
                &csrf,
            ),
        )
        .expect("bounded administrator free-appeal queue");
    }
    body.push_str("</div></section>");
    page("Апелляции по бесплатным комментариям", &body, &cookies)
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
        Ok(()) => super::private_response(Redirect::to("/odmen/free-comment-appeals"), &[]),
        Err(error) => deny(status(error)),
    }
}
