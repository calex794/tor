//! S049: administrator-only queue for paid-edition comment complaints.
//! Displays the reported E comment, never the private chapter itself.
use super::{deny, page, status, ShareState};
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use std::fmt::Write;
use tor_accounts::services::security::session_token_digest;
use tor_social::repositories::paid_comment_reviews::PgPaidCommentReviews;

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct QueueQuery {
    history_before: Option<i64>,
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

fn store(s: &ShareState) -> PgPaidCommentReviews {
    PgPaidCommentReviews::new(s.accounts.pool().clone())
}

pub(super) async fn queue(
    State(s): State<ShareState>,
    Query(query): Query<QueueQuery>,
    headers: HeaderMap,
) -> Response {
    let digest = match session_user(&s, &headers).await {
        Ok(digest) => digest,
        Err(error) => return deny(error),
    };
    let pending = match store(&s).pending(&digest, 50).await {
        Ok(pending) => pending,
        Err(error) => return deny(status(error)),
    };
    let history = match store(&s).history(&digest, query.history_before, 25).await {
        Ok(history) => history,
        Err(error) => return deny(status(error)),
    };
    let (csrf, cookies) = s.cookies.csrf_for_page(&headers);
    let mut body = String::from(
        r#"<section class="section"><div class="container">
        <h1>Жалобы на комментарии купленных глав</h1>
        <p>Доступно только администратору. Признание жалобы обоснованной не скрывает комментарий.
        Отдельное действие «Скрыть комментарий» относится только к указанной редакции.
        Покупка и текст главы не меняются.</p>"#,
    );
    if pending.is_empty() {
        body.push_str("<p>Ожидающих жалоб нет.</p>");
    }
    for item in pending {
        write!(
            body,
            r#"<article class="me-now-card">
            <h2>Жалоба #{}</h2>
            <p>Глава #{} · редакция #{} · комментарий #{} · заявитель #{}</p>
            <p>Причина: {}</p><p>Описание: {}</p>
            <blockquote>{}</blockquote>
            <form method="post" action="/admin/paid-comment-reports/{}/decision">
            <input type="hidden" name="csrf_token" value="{}">
            <label>Мотивировка решения
            <input name="note" maxlength="500" required></label>
            <button type="submit" name="decision" value="reviewed">Признать обоснованной</button>
            <button type="submit" name="decision" value="dismissed">Отклонить</button>
            <button type="submit" name="decision" value="hide_comment">Скрыть комментарий</button>
            </form></article>"#,
            item.id,
            item.chapter_id,
            item.version_id,
            item.comment_id,
            item.reporter_id,
            super::escape_html(&item.reason),
            super::escape_html(&item.details),
            super::escape_html(&item.comment_body),
            item.id,
            super::escape_html(&csrf),
        )
        .expect("bounded S049 paid comment review form");
    }
    body.push_str("<section aria-label=\"История решений\"><h2>История решений</h2>");
    if history.items.is_empty() {
        body.push_str("<p>История решений пока пуста.</p>");
    }
    for item in history.items {
        write!(
            body,
            r#"<article class="me-now-card">
            <h3>Решение по жалобе #{}</h3>
            <p>Комментарий #{} · администратор #{}</p>
            <p>Решение: {} · действие: {} · UTC {}</p><p>Мотивировка: {}</p>
            </article>"#,
            item.report_id,
            item.comment_id,
            item.reviewer_id,
            super::escape_html(&item.decision),
            super::escape_html(&item.moderation_action),
            super::escape_html(&item.decided_utc),
            super::escape_html(&item.note),
        )
        .expect("bounded S049 paid comment decision history");
    }
    if let Some(before) = history.next_cursor {
        write!(
            body,
            r#"<p><a href="/admin/paid-comment-reports?history_before={before}">
            Более ранние решения</a></p>"#,
        )
        .expect("bounded S049 paid comment cursor");
    }
    body.push_str("</section></div></section>");
    page("Жалобы на комментарии купленных глав", &body, &cookies)
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
        Ok(()) => super::private_response(Redirect::to("/admin/paid-comment-reports"), &[]),
        Err(error) => deny(status(error)),
    }
}
