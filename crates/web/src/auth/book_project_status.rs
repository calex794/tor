//! S020: editorial work status, never the book's public visibility.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, Redirect, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use tor_books::{repositories::postgres::OwnedProjectStatus, services::authoring::AuthorError};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct StatusForm {
    project_status: String,
    expected_revision: i64,
    csrf_token: Option<String>,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new().route(
        "/me/books/{id}/settings/project-status",
        get(edit).post(save),
    )
}

fn render(id: i64, book: &OwnedProjectStatus, csrf: &str) -> Html<String> {
    let mut choices = String::new();
    for (value, label) in [
        ("", "Не указан"),
        ("planned", "Планируется"),
        ("ongoing", "В работе"),
        ("paused", "Приостановлено"),
        ("completed", "Завершено"),
    ] {
        let selected = if book.status == value {
            " selected"
        } else {
            ""
        };
        choices.push_str(&format!(
            "<option value=\"{value}\"{selected}>{label}</option>"
        ));
    }
    let content = format!(
        r#"<section class="section auth-section"><div class="container">
        <div class="auth-card author-card">
        <h1>Состояние работы над книгой «{}»</h1>
        <p>Это редакционное состояние произведения. Оно не публикует черновик,
        не скрывает опубликованную книгу и не меняет купленные главы.</p>
        <form class="auth-form" method="post"
          action="/me/books/{id}/settings/project-status">
        <input type="hidden" name="csrf_token" value="{}">
        <input type="hidden" name="expected_revision" value="{}">
        <label for="project-status">Состояние работы</label>
        <select id="project-status" name="project_status">{choices}</select>
        <button class="button" type="submit">Сохранить</button>
        </form><p><a href="/me/books/{id}/settings">Все настройки книги</a></p>
        </div></div></section>"#,
        pages::escape_html(&book.title),
        pages::escape_html(csrf),
        book.revision,
    );
    pages::layout(
        "Состояние работы над книгой",
        "Редакторские настройки.",
        &content,
    )
}

pub(super) async fn edit(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let book = match state.authoring.manageable_project_status(actor, id).await {
        Ok(Some(book)) => book,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(render(id, &book, &csrf), &cookies)
}

pub(super) async fn save(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<StatusForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    match state
        .authoring
        .update_project_status(actor, id, form.expected_revision, &form.project_status)
        .await
    {
        Ok(_) => private_response(Redirect::to(&format!("/me/books/{id}/settings")), &[]),
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
    fn s020_status_form_escapes_title_and_has_no_visibility_switch() {
        let book = OwnedProjectStatus {
            title: "<script>alert(1)</script>".to_owned(),
            status: "paused".to_owned(),
            revision: 2,
        };
        let page = render(77, &book, "csrf").0;
        assert!(page.contains("&lt;script&gt;"));
        assert!(!page.contains("<script>alert(1)</script>"));
        assert!(page.contains("value=\"paused\" selected"));
        assert!(page.contains("name=\"expected_revision\" value=\"2\""));
        assert!(!page.contains("name=\"visibility\""));
        assert!(!page.contains("name=\"access_level\""));
    }
}
