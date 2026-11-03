//! Owner-only NEwBOO-style new chapter form.
//! The suggested number is a read-only hint, not an allocation or permission.
use super::{deny, session_actor, AuthorChapterState};
use crate::auth_cookies::private_response;
use axum::{
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use tor_accounts::services::security::generate_session_token;
use tor_books::services::authoring::AuthorError;

pub(super) async fn render(
    state: AuthorChapterState,
    id: i64,
    headers: HeaderMap,
) -> Response {
    let actor = match session_actor(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    match state.authoring.owned_book(actor, id).await {
        Ok(Some(_)) => (),
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    let is_public = match state.authoring.owned_book_is_public(actor, id).await {
        Ok(value) => value,
        Err(AuthorError::Missing) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let paid_enabled = is_public && state.paid_author.is_some();
    let next_number = match state.authoring.next_owned_chapter_number(actor, id).await {
        Ok(number) => number,
        Err(AuthorError::Missing) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let key = generate_session_token();
    private_response(
        crate::author_chapter_pages::new_chapter(
            id,
            &csrf,
            &key,
            paid_enabled,
            is_public,
            next_number,
        ),
        &cookies,
    )
}
