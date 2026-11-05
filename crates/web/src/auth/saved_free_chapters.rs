//! Authenticated, read-only private saved-free chapter navigation.
//! No public reader entitlement is inferred from a Book bookmark alone.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use tor_reading::services::progress::{reader_blocks, valid_text_place, ProgressError, ReadingPlace};
use tor_reading::{domain::renderer::valid_reader_length, services::library::LibraryError};

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/me/library/{id}/saved", get(list))
        .route("/me/library/saved/{id}", get(read))
        .route("/me/library/saved/{id}/place", post(save_position))
}

#[derive(Deserialize)]
pub(super) struct SavedFreeQuery {
    after: Option<i64>,
}

#[derive(Deserialize)]
pub(super) struct ArchivedPlaceForm {
    csrf_token: Option<String>,
    version_id: i64,
    block_index: i32,
    char_offset: i32,
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
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut response = private_response(pages::saved_free_reader(&saved, &csrf), &cookies);
    response.headers_mut().insert(
        "content-security-policy",
        "default-src 'none'; script-src 'none'; style-src 'self'; img-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'"
            .parse()
            .expect("static CSP"),
    );
    response
}


/// The only archived cursor writer. The exact old immutable edition is
/// verified both for rendering and again atomically by the storage layer.
pub(super) async fn save_position(
    State(state): State<AuthState>,
    Path(chapter_id): Path<i64>,
    headers: HeaderMap,
    axum::Form(form): axum::Form<ArchivedPlaceForm>,
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
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = tor_accounts::services::security::session_token_digest(&token);
    let chapter = match state
        .bookmarks
        .pinned_free_chapter(actor, chapter_id, &digest)
        .await
    {
        Ok(Some(chapter)) if valid_reader_length(&chapter.body_markdown) => chapter,
        Ok(Some(_)) => return deny(StatusCode::PAYLOAD_TOO_LARGE),
        Ok(None) | Err(LibraryError::Invalid | LibraryError::NotFound) => {
            return deny(StatusCode::NOT_FOUND);
        }
        Err(LibraryError::Unavailable) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if chapter.version_id != form.version_id {
        return deny(StatusCode::CONFLICT);
    }
    let place = ReadingPlace {
        block_index: form.block_index,
        char_offset: form.char_offset,
    };
    if valid_text_place(place, &reader_blocks(&chapter.body_markdown)).is_err() {
        return deny(StatusCode::BAD_REQUEST);
    }
    match state
        .progress
        .save_archived_position(actor, chapter_id, form.version_id, place, &digest)
        .await
    {
        Ok(()) => private_response(
            Redirect::to(&format!("/me/library/saved/{chapter_id}#saved-resume")),
            &[],
        ),
        Err(ProgressError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(ProgressError::NotFound) => deny(StatusCode::NOT_FOUND),
        Err(ProgressError::StaleVersion) => deny(StatusCode::CONFLICT),
        Err(ProgressError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}
