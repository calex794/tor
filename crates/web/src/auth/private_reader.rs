//! C/H: account-only free reading and the explicit continuation prompt.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use tor_reading::{domain::renderer::valid_reader_length, services::PublicReadingRepository};

/// Private verified reader; public free chapters use /reader/{id}.
pub(super) async fn page(
    State(state): State<AuthState>,
    Path(id): Path<u64>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let chapter = match state.reader.public_chapter(id).await {
        Ok(Some(chapter)) if valid_reader_length(&chapter.body_markdown) => chapter,
        Ok(Some(_)) => return deny(StatusCode::PAYLOAD_TOO_LARGE),
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (chapter_id, version_id) = match (i64::try_from(id), i64::try_from(chapter.version_id)) {
        (Ok(chapter_id), Ok(version_id)) => (chapter_id, version_id),
        _ => return deny(StatusCode::NOT_FOUND),
    };
    let saved = match state
        .progress
        .saved_state(actor, chapter_id, version_id)
        .await
    {
        Ok(saved) => saved,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    // A completion form uses the current per-book CAS revision, not a
    // client-invented one. A late second device will receive 409.
    let revision = match state.sync.free_state(actor, chapter_id).await {
        Ok(Some(progress)) => progress.receipt.revision,
        Ok(None) => 0,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let completed = match state.sync.completion_event_id(actor, chapter_id).await {
        Ok(event) => event.is_some(),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let demand = if completed && chapter.next_chapter_id.is_none() {
        let book_id = match i64::try_from(chapter.book_id) {
            Ok(value) => value,
            Err(_) => return deny(StatusCode::NOT_FOUND),
        };
        match state.demand.state(actor, book_id, chapter_id).await {
            Ok(status) => Some(status),
            Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        }
    } else {
        None
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(
        pages::private_reader_with_demand(id, &chapter, &csrf, saved, revision, completed, demand),
        &cookies,
    )
}

