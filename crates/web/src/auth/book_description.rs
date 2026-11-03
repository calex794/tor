//! S020: authenticated active Book members may edit annotations.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Html, Redirect, Response},
};
use serde::Deserialize;
use tor_books::{repositories::postgres::ManagedBookDescription, services::authoring::AuthorError};

#[derive(Deserialize)]
pub(super) struct DescriptionForm {
    description: String,
    csrf_token: Option<String>,
}

fn render(book_id: i64, book: &ManagedBookDescription, csrf: &str) -> Html<String> {
    let content = format!(
        r#"<section class="section auth-section"><div class="container"><div class="auth-card author-card">
        <h1>Аннотация книги «{}»</h1>
        <form class="auth-form" method="post" action="/me/books/{book_id}/description">
        <input type="hidden" name="csrf_token" value="{}">
        <label for="book-description">Аннотация (до 4000 символов)</label>
        <textarea id="book-description" name="description" rows="9" maxlength="4000">{}</textarea>
        <button class="button" type="submit">Сохранить аннотацию</button>
        </form><p><a href="/me/books/{book_id}/settings">Все настройки книги</a></p>
        </div></div></section>"#,
        pages::escape_html(&book.title),
        pages::escape_html(csrf),
        pages::escape_html(&book.description),
    );
    pages::layout("Аннотация книги", "Редактирование аннотации.", &content)
}

pub(super) async fn edit(
    State(state): State<AuthState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let book = match state
        .authoring
        .manageable_book_description(actor, book_id)
        .await
    {
        Ok(Some(book)) => book,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(render(book_id, &book, &csrf), &cookies)
}

pub(super) async fn save(
    State(state): State<AuthState>,
    Path(book_id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<DescriptionForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    match state
        .authoring
        .update_book_description(actor, book_id, &form.description)
        .await
    {
        Ok(()) => private_response(Redirect::to(&format!("/me/books/{book_id}/settings")), &[]),
        Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(AuthorError::Forbidden) => deny(StatusCode::FORBIDDEN),
        Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Conflict) => deny(StatusCode::CONFLICT),
        Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn s020_description_editor_escapes_html_and_links_to_member_settings() {
        let book = ManagedBookDescription {
            title: "<script>bad</script>".to_owned(),
            description: "<img src=x onerror=alert(1)>".to_owned(),
        };
        let html = render(52, &book, "csrf").0;
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;img"));
        assert!(!html.contains("<img"));
        assert!(html.contains("/me/books/52/settings"));
        assert!(!html.contains("name=\"visibility\""));
    }
}
