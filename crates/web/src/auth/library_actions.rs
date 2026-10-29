//! Browser and API adapters for C-08 personal bookmarks.
#![forbid(unsafe_code)]

use super::{deny, session_user, AuthState, LibraryQuery};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use tor_reading::services::{library::LibraryError, PublicReadingRepository};

#[derive(Deserialize)]
pub(super) struct BookmarkForm {
    csrf_token: Option<String>,
}

#[derive(Serialize)]
struct LibraryItemJson {
    book_id: String,
    title: String,
}

#[derive(Serialize)]
struct LibraryJson {
    items: Vec<LibraryItemJson>,
    next_cursor: Option<String>,
}

fn library_status(error: LibraryError) -> StatusCode {
    match error {
        LibraryError::Invalid => StatusCode::BAD_REQUEST,
        LibraryError::NotFound => StatusCode::NOT_FOUND,
        LibraryError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

/// Private combined library: continue reading, saved Books and opt-in Layer history.
pub(super) async fn browser_library(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<LibraryQuery>,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let books = match state.progress.continue_reading(actor).await {
        Ok(books) => books,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let saved = match state
        .bookmarks
        .list_books(actor, query.limit.unwrap_or(30), query.after)
        .await
    {
        Ok(saved) => saved,
        Err(LibraryError::Invalid) => return deny(StatusCode::BAD_REQUEST),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(
        pages::my_library_with_options(
            &books,
            &saved,
            &csrf,
            state.paid_reader_available,
            state.layer_pages_enabled,
        ),
        &cookies,
    )
}

pub(super) async fn api_list(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<LibraryQuery>,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    match state
        .bookmarks
        .list_books(actor, query.limit.unwrap_or(30), query.after)
        .await
    {
        Ok(page) => {
            let items = page
                .items
                .into_iter()
                .map(|book| LibraryItemJson {
                    book_id: book.book_id.to_string(),
                    title: book.title,
                })
                .collect();
            private_response(
                Json(LibraryJson {
                    items,
                    next_cursor: page.next_cursor.map(|id| id.to_string()),
                }),
                &[],
            )
        }
        Err(error) => deny(library_status(error)),
    }
}

pub(super) async fn show_add(
    State(state): State<AuthState>,
    Path(id): Path<u64>,
    headers: HeaderMap,
) -> Response {
    match session_user(&state, &headers).await {
        Ok(Some(_)) => (),
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    }
    let book = match state.reader.public_book(id).await {
        Ok(Some(book)) => book,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(pages::bookmark_prompt(&book, &csrf), &cookies)
}

async fn write(
    state: &AuthState,
    headers: &HeaderMap,
    id: u64,
    form: &BookmarkForm,
    add: bool,
) -> Result<(), StatusCode> {
    if !state.cookies.same_origin(headers)
        || !state
            .cookies
            .valid_csrf(headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return Err(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(state, headers).await? {
        Some(actor) => actor,
        None => return Err(StatusCode::UNAUTHORIZED),
    };
    let book_id = i64::try_from(id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let result = if add {
        state.bookmarks.add_book(actor, book_id).await
    } else {
        state.bookmarks.remove_book(actor, book_id).await
    };
    result.map_err(library_status)
}

pub(super) async fn api_add(
    State(state): State<AuthState>,
    Path(id): Path<u64>,
    headers: HeaderMap,
    Form(form): Form<BookmarkForm>,
) -> Response {
    match write(&state, &headers, id, &form, true).await {
        Ok(()) => private_response(StatusCode::NO_CONTENT, &[]),
        Err(status) => deny(status),
    }
}

pub(super) async fn api_delete(
    State(state): State<AuthState>,
    Path(id): Path<u64>,
    headers: HeaderMap,
    Form(form): Form<BookmarkForm>,
) -> Response {
    match write(&state, &headers, id, &form, false).await {
        Ok(()) => private_response(StatusCode::NO_CONTENT, &[]),
        Err(status) => deny(status),
    }
}

pub(super) async fn browser_add(
    State(state): State<AuthState>,
    Path(id): Path<u64>,
    headers: HeaderMap,
    Form(form): Form<BookmarkForm>,
) -> Response {
    match write(&state, &headers, id, &form, true).await {
        Ok(()) => private_response(Redirect::to("/me/library"), &[]),
        Err(status) => deny(status),
    }
}

pub(super) async fn browser_remove(
    State(state): State<AuthState>,
    Path(id): Path<u64>,
    headers: HeaderMap,
    Form(form): Form<BookmarkForm>,
) -> Response {
    match write(&state, &headers, id, &form, false).await {
        Ok(()) => private_response(Redirect::to("/me/library"), &[]),
        Err(status) => deny(status),
    }
}
