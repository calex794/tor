//! Authenticated, read-only private saved-free chapter navigation.
//! No public reader entitlement is inferred from a Book bookmark alone.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use tor_reading::{domain::renderer::valid_reader_length, services::library::LibraryError};

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/me/library/{id}/saved", get(list))
        .route("/me/library/saved/{id}", get(read))
}

#[derive(Deserialize)]
pub(super) struct SavedFreeQuery {
    after: Option<i64>,
}

pub(super) async fn list(
    State(state): State<AuthState>,
    Path(book_id): Path<i64>,
    Query(query): Query<SavedFreeQuery>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let session_digest = tor_accounts::services::security::session_token_digest(&token);
    let result = state
        .bookmarks
        .saved_archive_chapters(actor, book_id, 50, query.after, &session_digest)
        .await;
    match result {
        Ok(page) => private_response(
            pages::saved_archive_list(book_id, &page, state.paid_reader_available),
            &[],
        ),
        Err(LibraryError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

pub(super) async fn read(
    State(state): State<AuthState>,
    Path(chapter_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let session_digest = tor_accounts::services::security::session_token_digest(&token);
    let saved = match state
        .bookmarks
        .pinned_free_chapter(actor, chapter_id, &session_digest)
        .await
    {
        Ok(Some(saved)) if valid_reader_length(&saved.body_markdown) => saved,
        Ok(Some(_)) => return deny(StatusCode::PAYLOAD_TOO_LARGE),
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(LibraryError::Invalid | LibraryError::NotFound) => {
            return deny(StatusCode::NOT_FOUND);
        }
        Err(LibraryError::Unavailable) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let mut response = private_response(pages::saved_free_reader(&saved), &[]);
    response.headers_mut().insert(
        "content-security-policy",
        "default-src 'none'; script-src 'none'; style-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'"
            .parse()
            .expect("static CSP"),
    );
    response
}
