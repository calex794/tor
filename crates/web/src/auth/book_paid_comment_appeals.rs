//! S049: author appeal and independent administrator review for hidden paid V1 comments.
//! Never returns the sealed text, grants, purchased versions, or selected quotes.
use super::{
    appeal_markup::{author_action, AppealForm, AppealTarget, DecisionForm},
    deny, page, session_user, status, ShareState,
};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use std::fmt::Write;
use tor_social::repositories::paid_comment_appeals::PgPaidCommentAppeals;

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
        body.push_str(&author_action(
            AppealTarget::Paid,
            item.hide_event_id,
            item.appeal_state.as_deref(),
            &item.visibility,
            &csrf,
        ));
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
        <p><a href="/odmen/paid-comment-reports">К очереди жалоб</a></p>"#,
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
                super::appeal_markup::AppealTarget::Paid,
                item.id,
                &csrf,
            ),
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
        Ok(()) => super::private_response(Redirect::to("/odmen/paid-comment-appeals"), &[]),
        Err(error) => deny(status(error)),
    }
}
