//! S020: active book members edit non-deletion lifecycle metadata only.
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
use tor_books::{repositories::postgres::ManagedBookLifecycle, services::authoring::AuthorError};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LifecycleForm {
    lifecycle_status: String,
    expected_revision: i64,
    csrf_token: Option<String>,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new().route("/me/books/{id}/settings/lifecycle", get(edit).post(save))
}

fn render(id: i64, book: &ManagedBookLifecycle, csrf: &str) -> Html<String> {
    let mut choices = String::new();
    for (value, label) in [
        ("in_progress", "В процессе"),
        ("completed", "Завершено"),
        ("on_hiatus", "На паузе"),
        ("abandoned", "Заброшено"),
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
    let editor = if book.status == "pending_deletion" {
        String::from(
            "<p>Книга ожидает удаления. Изменение этого состояния требует отдельного защищённого действия.</p>",
        )
    } else {
        format!(
            r#"<form class="auth-form" method="post"
                  action="/me/books/{id}/settings/lifecycle">
              <input type="hidden" name="csrf_token" value="{}">
              <input type="hidden" name="expected_revision" value="{}">
              <label for="lifecycle-status">Жизненный цикл</label>
              <select id="lifecycle-status" name="lifecycle_status">{choices}</select>
              <button class="button" type="submit">Сохранить</button>
              </form>"#,
            pages::escape_html(csrf),
            book.revision,
        )
    };
    let content = format!(
        r#"<section class="section auth-section"><div class="container">
        <div class="auth-card author-card">
        <h1>Жизненный цикл книги «{}»</h1>
        <p>Статус произведения не публикует и не скрывает книгу,
        не меняет возрастной уровень, главы, покупки или редакционный статус.</p>
        {editor}
        <p><a href="/me/books/{id}">Вернуться к книге</a></p>
        </div></div></section>"#,
        pages::escape_html(&book.title),
    );
    pages::layout("Жизненный цикл книги", "Настройки произведения.", &content)
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
    let book = match state.authoring.manageable_lifecycle(actor, id).await {
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
    Form(form): Form<LifecycleForm>,
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
        .update_lifecycle(actor, id, form.expected_revision, &form.lifecycle_status)
        .await
    {
        Ok(_) => private_response(
            Redirect::to(&format!("/me/books/{id}/settings/lifecycle")),
            &[],
        ),
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
    fn s020_lifecycle_form_escapes_title_and_never_offers_deletion() {
        let book = ManagedBookLifecycle {
            title: "<script>alert(1)</script>".into(),
            status: "on_hiatus".into(),
            revision: 4,
        };
        let rendered = render(42, &book, "csrf").0;
        assert!(rendered.contains("&lt;script&gt;"));
        assert!(!rendered.contains("<script>alert(1)</script>"));
        assert!(rendered.contains("value=\"on_hiatus\" selected"));
        assert!(rendered.contains("name=\"expected_revision\" value=\"4\""));
        assert!(!rendered.contains("name=\"visibility\""));
        assert!(!rendered.contains("name=\"access_level\""));
        assert!(!rendered.contains("value=\"pending_deletion\""));
        let pending = ManagedBookLifecycle {
            status: "pending_deletion".into(),
            ..book
        };
        let protected = render(42, &pending, "csrf").0;
        assert!(!protected.contains("method=\"post\""));
    }
}
