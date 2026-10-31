//! Authenticated reader library with explicit new-chapter updates.
use super::{deny, followed_purchase_links, pages, session_token_digest, AuthState, LibraryQuery};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};

pub(super) async fn my_library(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<LibraryQuery>,
) -> Response {
    let Some(token) = state.cookies.session_token(&headers) else {
        return private_response(Redirect::to("/login"), &[]);
    };
    let digest = session_token_digest(&token);
    let user_id = match state.accounts.active_session_user(&digest).await {
        Ok(Some(id)) => id,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let books = match state.progress.continue_reading(user_id).await {
        Ok(books) => books,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let saved = match state
        .bookmarks
        .list_books(user_id, query.limit.unwrap_or(30), query.after)
        .await
    {
        Ok(saved) => saved,
        Err(tor_reading::services::library::LibraryError::Invalid) => {
            return deny(StatusCode::BAD_REQUEST);
        }
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let cursor = match query.updates_after.as_deref() {
        Some(raw) => match tor_reading::repositories::UpdatesCursor::parse(raw) {
            Some(cursor) => Some(cursor),
            None => return deny(StatusCode::BAD_REQUEST),
        },
        None => None,
    };
    let updates = match state.follow_updates.latest(user_id, cursor).await {
        Ok(updates) => updates,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let revision_cursor = match query.revisions_after.as_deref() {
        Some(value) => match tor_reading::repositories::RevisionCursor::parse(value) {
            Some(cursor) => Some(cursor),
            None => return deny(StatusCode::BAD_REQUEST),
        },
        None => None,
    };
    let revisions = match state
        .staged_revisions
        .latest(user_id, revision_cursor)
        .await
    {
        Ok(revisions) => revisions,
        Err(tor_reading::repositories::StagedRevisionError::Invalid) => {
            return deny(StatusCode::BAD_REQUEST);
        }
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let owned = match followed_purchase_links::for_reader(&state, user_id, &updates).await {
        Ok(owned) => owned,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let page = pages::my_library_with_updates(
        &books,
        &saved,
        &csrf,
        state.paid_reader_available,
        state.paid_checkout_enabled,
        (&updates, &revisions),
        &owned,
    );
    private_response(page, &cookies)
}
