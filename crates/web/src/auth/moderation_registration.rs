//! MOD-REG-01: site-level Book registration queue, not chapter moderation.
use super::{deny, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Html, Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use tor_accounts::{
    repositories::{
        moderation_books::ModerationBooksError, moderation_registration::RegistrationCandidate,
    },
    services::security::session_token_digest,
};

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RegistrationQuery {
    before: Option<i64>,
    q: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RegistrationDecisionForm {
    csrf_token: String,
    expected_revision: i64,
    action: String,
    reason: Option<String>,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/moderation/books/registrations", get(queue))
        .route("/moderation/books/{id}/registration", post(decide))
        .route("/moderation/books/{id}/registration-history", get(history))
}

fn error_code(e: ModerationBooksError) -> StatusCode {
    match e {
        ModerationBooksError::Denied => StatusCode::FORBIDDEN,
        ModerationBooksError::Invalid => StatusCode::BAD_REQUEST,
        ModerationBooksError::Missing => StatusCode::NOT_FOUND,
        ModerationBooksError::Stale => StatusCode::CONFLICT,
        ModerationBooksError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

fn render(
    items: &[RegistrationCandidate],
    count: i64,
    csrf: &str,
    covers_count: i64,
    deletions_count: i64,
    search: Option<&str>,
) -> String {
    let token = pages::escape_html(csrf);
    let search = pages::escape_html(search.unwrap_or(""));
    let mut cards = String::new();
    for item in items {
        let title = pages::escape_html(&item.title);
        let owner = item
            .owner_user_id
            .map_or_else(|| "—".to_owned(), |id| id.to_string());
        cards.push_str(&format!(
            r#"<article class="me-now-card">
            <h2>{title}</h2>
            <p>Книга #{book} · Владелец #{owner} · Ревизия {rev}</p>
            <p><a href="/moderation/books/{book}/history">Все решения модерации</a></p>
            <details class="moderation-review-confirm" open>
            <summary>Подтвердить решение по регистрации книги #{book}</summary>
            <p>Проверьте название и ревизию. Одобрение не публикует авторский черновик.</p>
            <form method="post" action="/moderation/books/{book}/registration">
              <input type="hidden" name="csrf_token" value="{token}">
              <input type="hidden" name="expected_revision" value="{rev}">
              <label for="registration-reason-{book}">Причина отказа (обязательна)</label>
              <textarea id="registration-reason-{book}" name="reason"
                        maxlength="2000" rows="2"></textarea>
              <button type="submit" name="action" value="approve">Одобрить регистрацию</button>
              <button type="submit" name="action" value="reject">Отклонить</button>
            </form></details></article>"#,
            book = item.book_id,
            rev = item.revision,
        ));
    }
    if cards.is_empty() {
        cards.push_str("<p>Нет книг, ожидающих регистрацию.</p>");
    }
    if items.len() == 40 {
        if let Some(last) = items.last() {
            cards.push_str(&format!(
                r#"<form method="get" action="/moderation/books/registrations">
                <input type="hidden" name="before" value="{}">
                <input type="hidden" name="q" value="{search}">
                <button type="submit">Следующие заявки</button></form>"#,
                last.book_id
            ));
        }
    }
    pages::layout(
        "Модерация регистрации книг",
        "Закрытая очередь утверждения регистрации.",
        &format!(
            r#"<section class="me-page"><div class="container">
            <h1>Кабинет модератора</h1>
            <nav aria-label="Очереди модератора">
              <a href="/moderation/books/registrations" aria-current="page">Регистрация ({count})</a>
              <a href="/moderation/covers">Обложки ({covers_count})</a>
              <a href="/moderation/books/deletions">Удаление ({deletions_count})</a>
              <a href="/moderation/books/control">Контроль</a>
            </nav>
              <p><a href="/moderation/books/reviews">Журнал рассмотренных заявок</a></p>
            <form method="get" action="/moderation/books/registrations">
              <label for="registration-search">Поиск по названию или ID книги</label>
              <input id="registration-search" name="q" maxlength="120" value="{search}">
              <button type="submit">Найти</button>
              <a href="/moderation/books/registrations">Сбросить</a>
            </form>
            <p>Одобрение регистрации не публикует авторский черновик,
               а новые главы не проходят отдельную модерацию.</p>
            {cards}
            <p><a href="/moderation/books/control">Контроль и удержания книг</a></p>
            </div></section>"#
        ),
    )
    .0
}

pub(super) async fn queue(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<RegistrationQuery>,
) -> Response {
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.moderator || actor.administrator => {}
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let count = match state
        .accounts
        .moderation_registration_pending_count(&digest)
        .await
    {
        Ok(count) => count,
        Err(e) => return deny(error_code(e)),
    };
    let covers_count = match tor_books::repositories::cover_review::PgCoverReview::new(
        state.accounts.pool().clone(),
    )
    .pending_count(&digest)
    .await
    {
        Ok(count) => count,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let deletions_count = match state
        .accounts
        .moderation_pending_deletion_count(&digest)
        .await
    {
        Ok(count) => count,
        Err(e) => return deny(error_code(e)),
    };
    let rows = match state
        .accounts
        .moderation_registration_queue_filtered(&digest, query.before, query.q.as_deref())
        .await
    {
        Ok(rows) => rows,
        Err(e) => return deny(error_code(e)),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut response = private_response(
        Html(render(
            &rows,
            count,
            &csrf,
            covers_count,
            deletions_count,
            query.q.as_deref(),
        )),
        &cookies,
    );
    response.headers_mut().insert(
        "x-robots-tag",
        HeaderValue::from_static("noindex, nofollow"),
    );
    response
}

pub(super) async fn decide(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<RegistrationDecisionForm>,
) -> Response {
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.moderator || actor.administrator => {}
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    match state
        .accounts
        .moderate_book_registration(
            &digest,
            id,
            form.expected_revision,
            &form.action,
            form.reason.as_deref().unwrap_or(""),
        )
        .await
    {
        Ok(()) => private_response(Redirect::to("/moderation/books/registrations"), &[]),
        Err(e) => super::decision_error(error_code(e), "/moderation/books/registrations"),
    }
}

pub(super) async fn history(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) if actor.moderator || actor.administrator => {}
        Ok(Some(_)) => return deny(StatusCode::FORBIDDEN),
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let approval = match state
        .accounts
        .private_book_registration_approver(&digest, id)
        .await
    {
        Ok(item) => item,
        Err(e) => return deny(error_code(e)),
    };
    let info = match approval {
        Some(item) => format!(
            "<p>Первоначально одобрил модератор: {} (#{}) · время Unix {}</p>",
            pages::escape_html(&item.reviewer_name),
            item.reviewer_user_id,
            item.approved_unix
        ),
        None => "<p>Подтверждённое первоначальное одобрение отсутствует.</p>".to_owned(),
    };
    private_response(
        pages::layout(
            "История регистрации книги",
            "Только модераторы сайта и администраторы.",
            &format!(
                "<section class=\"me-page\"><div class=\"container\">
                 <h1>Системная история регистрации книги #{id}</h1>{info}
                 <a href=\"/moderation/books/registrations\">Очередь регистрации</a>
                 </div></section>"
            ),
        ),
        &[],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn queue_hides_private_metadata_from_public_book() {
        let rows = vec![RegistrationCandidate {
            book_id: 51,
            title: "<script>private</script>".into(),
            owner_user_id: Some(37),
            revision: 1,
        }];
        let body = render(&rows, 1, "csrf", 2, 3, None);
        assert!(body.contains("&lt;script&gt;private&lt;/script&gt;"));
        assert!(!body.contains("<script>"));
        assert!(body.contains("Регистрация (1)"));
        assert!(body.contains("moderation-review-confirm"));
        assert!(body.contains("name=\"q\""));
        assert!(body.contains("Обложки (2)"));
        assert!(body.contains("Удаление (3)"));
        assert!(body.contains("/moderation/books/51/registration"));
        assert!(!body.contains("reviewer_user_id"));
        let response = private_response(Html(body), &[]);
        assert_eq!(
            response.headers()[axum::http::header::CONTENT_TYPE],
            "text/html; charset=utf-8",
            "moderator queue must render functional forms in Chromium"
        );
    }
}
