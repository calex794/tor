//! BR-060: owner-only password-confirmed ordinary Book removal.
//! Never deletes Book, chapter, reader pin, purchase or financial history.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{DefaultBodyLimit, Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, Redirect, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use tor_accounts::services::security::{session_token_digest, verify_password};
use tor_books::{repositories::postgres::OrdinaryDeletionView, services::authoring::AuthorError};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct DeletionForm {
    action: String,
    expected_revision: i64,
    account_password: String,
    csrf_token: Option<String>,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new().route(
        "/me/books/{id}/settings/delete",
        get(page).post(submit).layer(DefaultBodyLimit::max(2048)),
    )
}

fn render(id: i64, book: &OrdinaryDeletionView, csrf: &str) -> Html<String> {
    let title = pages::escape_html(&book.title);
    let (action, label, warning) = match book.status.as_str() {
        "pending_deletion" if !book.moderator_approved => (
            "",
            "",
            "Заявка направлена на модерацию. Окончательное удаление невозможно
             до одобрения отдельным модератором сайта.",
        ),
        "pending_deletion" => (
            "complete",
            "Окончательно удалить книгу",
            "Книга ожидает удаления. Завершение уберёт её из публичного доступа. \
             Ранее сохранённые бесплатные редакции и действующие покупки \
             останутся в личных библиотеках соответствующих читателей.",
        ),
        "ordinary_deleted" => ("", "", "Книга удалена из публичного доступа."),
        _ => (
            "start",
            "Начать удаление",
            "Вы начинаете удаление книги. Для окончательного завершения \
             потребуется отдельное подтверждение паролем. \
             Ранее сохранённые главы и действующие покупки не стираются.",
        ),
    };
    let form = if action.is_empty() {
        String::new()
    } else {
        format!(
            "<form class=\"auth-form\" method=\"post\" action=\"/me/books/{id}/settings/delete\">
             <input type=\"hidden\" name=\"csrf_token\" value=\"{}\">
             <input type=\"hidden\" name=\"expected_revision\" value=\"{}\">
             <input type=\"hidden\" name=\"action\" value=\"{action}\">
             <label for=\"deletion-password\">Подтвердите пароль аккаунта</label>
             <input id=\"deletion-password\" type=\"password\" name=\"account_password\"
               autocomplete=\"current-password\" required>
             <button class=\"button\" type=\"submit\">{label}</button>
             </form>",
            pages::escape_html(csrf),
            book.revision,
        )
    };
    let html = format!(
        "<section class=\"section auth-section\"><div class=\"container\">
         <div class=\"auth-card\"><h1>Удаление книги «{title}»</h1>
         <p>{warning}</p>{form}
         <p><a href=\"/me/books\">Мои книги</a></p></div></div></section>"
    );
    pages::layout(
        "Удаление книги",
        "Защищённое действие владельца книги.",
        &html,
    )
}

pub(super) async fn page(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    match state.authoring.ordinary_deletion_view(actor, id).await {
        Ok(Some(book)) => {
            let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
            private_response(render(id, &book, &csrf), &cookies)
        }
        Ok(None) => deny(StatusCode::NOT_FOUND),
        Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<DeletionForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    if id <= 0
        || form.expected_revision <= 0
        || !matches!(form.action.as_str(), "start" | "complete")
        || form.account_password.is_empty()
        || form.account_password.len() > 1024
    {
        return deny(StatusCode::BAD_REQUEST);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    // Rate-limit account+operation, never the Book ID from the client.
    let key = session_token_digest(&format!("auth:ordinary-delete:{actor}"));
    match state.accounts.permit_auth_attempt(&key, 5, 600).await {
        Ok(true) => (),
        Ok(false) => return deny(StatusCode::TOO_MANY_REQUESTS),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let expected_hash = match state.accounts.password_hash_for_user(actor).await {
        Ok(Some(hash)) => hash,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let password = form.account_password;
    let hash_for_worker = expected_hash.clone();
    let confirmed = match state
        .password_jobs
        .run(move || verify_password(&password, &hash_for_worker))
        .await
    {
        Ok(valid) => valid,
        Err(status) => return deny(status),
    };
    if !confirmed {
        return deny(StatusCode::UNAUTHORIZED);
    }
    match state
        .authoring
        .advance_ordinary_deletion(
            actor,
            id,
            form.expected_revision,
            &form.action,
            &digest,
            &expected_hash,
        )
        .await
    {
        Ok(_) => {
            let _ = state.accounts.clear_auth_attempt(&key).await;
            private_response(
                Redirect::to(&format!("/me/books/{id}/settings/delete")),
                &[],
            )
        }
        Err(AuthorError::Conflict) => deny(StatusCode::CONFLICT),
        Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(AuthorError::Missing | AuthorError::Forbidden) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn deletion_form_requires_distinct_confirmations_and_escapes_title() {
        let book = OrdinaryDeletionView {
            title: "<script>x</script>".into(),
            status: "in_progress".into(),
            revision: 3,
            moderator_approved: false,
        };
        let start = render(19, &book, "csrf").0;
        assert!(start.contains("value=\"start\""));
        assert!(start.contains("name=\"account_password\""));
        assert!(start.contains("&lt;script&gt;"));
        assert!(!start.contains("<script>x</script>"));
        let pending = render(
            19,
            &OrdinaryDeletionView {
                status: "pending_deletion".into(),
                ..book.clone()
            },
            "csrf",
        )
        .0;
        assert!(!pending.contains("value=\"complete\""));
        assert!(pending.contains("Заявка направлена на модерацию"));
        let approved = render(
            19,
            &OrdinaryDeletionView {
                status: "pending_deletion".into(),
                moderator_approved: true,
                ..book.clone()
            },
            "csrf",
        )
        .0;
        assert!(approved.contains("value=\"complete\""));
        let done = render(
            19,
            &OrdinaryDeletionView {
                status: "ordinary_deleted".into(),
                ..book
            },
            "csrf",
        )
        .0;
        assert!(done.contains("удалена"));
        assert!(!done.contains("type=\"password\""));
    }
}
