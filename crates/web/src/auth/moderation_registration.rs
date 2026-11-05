//! MOD-REG-01: site-level Book registration queue, not chapter moderation.
use super::{deny, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{Redirect, Response},
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

fn render(items: &[RegistrationCandidate], count: i64, csrf: &str) -> String {
    let token = pages::escape_html(csrf);
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
            <form method="post" action="/moderation/books/{book}/registration">
              <input type="hidden" name="csrf_token" value="{token}">
              <input type="hidden" name="expected_revision" value="{rev}">
              <label for="registration-reason-{book}">Причина отказа (обязательна)</label>
              <textarea id="registration-reason-{book}" name="reason"
                        maxlength="2000" rows="2"></textarea>
              <button type="submit" name="action" value="approve">Одобрить регистрацию</button>
              <button type="submit" name="action" value="reject">Отклонить</button>
            </form></article>"#,
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
                r#"<a href="/moderation/books/registrations?before={}">Следующие заявки</a>"#,
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
              <a href="/moderation/covers">Обложки</a>
              <a href="/moderation/books/deletions">Удаление</a>
            </nav>
            <p>Одобрение регистрации не публикует авторский черновик,
               а новые главы не проходят отдельную модерацию.</p>
            {cards}
            <p><a href="/moderation/books">Контроль и удержания книг</a></p>
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
    let rows = match state
        .accounts
        .moderation_registration_queue(&digest, query.before)
        .await
    {
        Ok(rows) => rows,
        Err(e) => return deny(error_code(e)),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut response = private_response(render(&rows, count, &csrf), &cookies);
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
        Err(e) => deny(error_code(e)),
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
        let body = render(&rows, 1, "csrf");
        assert!(body.contains("&lt;script&gt;private&lt;/script&gt;"));
        assert!(!body.contains("<script>"));
        assert!(body.contains("Регистрация (1)"));
        assert!(body.contains("/moderation/books/51/registration"));
        assert!(!body.contains("reviewer_user_id"));
    }
}
