//! Only the authenticated owner may edit a book description.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_books::services::authoring::AuthorError;

#[derive(Deserialize)]
pub(super) struct DescriptionForm {
    description: String,
    csrf_token: Option<String>,
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
    let book = match state.authoring.owned_book(actor, book_id).await {
        Ok(Some(book)) => book,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let description = match state.authoring.owned_book_description(actor, book_id).await {
        Ok(Some(text)) => text,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let content = format!(
        r#"<section class="section auth-section"><div class="container"><div class="auth-card author-card">
        <p class="eyebrow">Кабинет автора</p><h1>Аннотация книги «{}»</h1>
        <form class="auth-form" method="post" action="/me/books/{book_id}/description">
        <input type="hidden" name="csrf_token" value="{}">
        <label for="book-description">Аннотация (до 4000 символов)</label>
        <textarea id="book-description" name="description" rows="9" maxlength="4000">{}</textarea>
        <button class="button" type="submit">Сохранить аннотацию</button>
        </form><p><a href="/me/books/{book_id}">Вернуться к книге</a></p>
        </div></div></section>"#,
        pages::escape_html(&book.title),
        pages::escape_html(&csrf),
        pages::escape_html(&description),
    );
    private_response(
        pages::layout("Аннотация книги", "Редактирование аннотации.", &content),
        &cookies,
    )
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
        Ok(()) => private_response(Redirect::to(&format!("/me/books/{book_id}")), &[]),
        Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(AuthorError::Forbidden) => deny(StatusCode::FORBIDDEN),
        Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Conflict) => deny(StatusCode::CONFLICT),
        Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}
