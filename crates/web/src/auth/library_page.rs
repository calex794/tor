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
    // A paid Book need not have a bookmark: purchases are independent facts.
    // Query the same live age, session, moderation and D purchase policies.
    let purchase_books = match state
        .bookmarks
        .purchased_only_books(user_id, &digest, 30, query.purchased_after)
        .await
    {
        Ok(page) => page,
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
    let owned = match followed_purchase_links::for_reader(&state, user_id, &updates).await {
        Ok(owned) => owned,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    // Resolve only covers attached to currently public, approved BASIC books.
    // Missing or unavailable slots render the illustrated fallback, not a broken image.
    let mut cover_book_ids = books
        .iter()
        .map(|book| book.book_id)
        .chain(saved.items.iter().map(|book| book.book_id))
        .chain(purchase_books.items.iter().map(|book| book.book_id))
        .chain(updates.items.iter().map(|release| release.book_id))
        .filter_map(|id| u64::try_from(id).ok())
        .collect::<Vec<_>>();
    cover_book_ids.sort_unstable();
    cover_book_ids.dedup();
    let mut confirmed_covers = std::collections::HashSet::new();
    if state.cover_preview_storage.is_some() {
        let slots =
            tor_books::repositories::cover_slots::PgCoverSlots::new(state.accounts.pool().clone());
        for batch in cover_book_ids.chunks(100) {
            match slots.public_card_ids(batch).await {
                Ok(ids) => confirmed_covers.extend(ids),
                Err(_) => {
                    // Optional artwork failure must not prevent reading.
                    confirmed_covers.clear();
                    break;
                }
            }
        }
    }
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    // Query-selected panels work with the strict script-src none CSP.
    // Preserve legacy cursors by deriving the panel when view is omitted.
    let view = query.view.as_deref().or_else(|| {
        if query.updates_after.is_some() {
            Some("updates")
        } else if query.purchased_after.is_some() {
            Some("purchased")
        } else if query.after.is_some() {
            Some("saved")
        } else {
            None
        }
    });
    let page = pages::my_library_with_updates_and_layers_and_purchased_only_selected(
        &books,
        &saved,
        &csrf,
        (state.paid_reader_available, state.layer_pages_enabled),
        state.paid_checkout_enabled,
        (&updates, &owned),
        (&purchase_books, view, Some(&confirmed_covers)),
    );
    private_response(page, &cookies)
}
