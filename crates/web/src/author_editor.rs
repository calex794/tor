//! B-03: browser author chapter and explicit book-release forms on real PG.
#![forbid(unsafe_code)]

use crate::{
    auth_cookies::{private_response, CookiePolicy},
    author_pages,
};
use axum::{
    extract::{DefaultBodyLimit, Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{generate_session_token, session_token_digest},
};
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::authoring::{save_chapter, AuthorError, SaveChapterRequest},
};

#[derive(Clone)]
struct EditorState {
    accounts: PgAccounts,
    authoring: PgAuthoring,
    cookies: CookiePolicy,
}

#[derive(Deserialize)]
struct ChapterForm {
    number: i32,
    title: String,
    body: String,
    command_key: String,
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
struct ReleaseForm {
    csrf_token: Option<String>,
}

pub(crate) fn router(accounts: PgAccounts, secure: bool, origin: String) -> Router {
    let state = EditorState {
        authoring: PgAuthoring::new(accounts.pool().clone()),
        accounts,
        cookies: CookiePolicy { secure, origin },
    };
    Router::new()
        .route(
            "/me/books/{id}/chapters/new",
            get(chapter_form).post(create_chapter),
        )
        .route("/me/books/{id}/release", get(release_form).post(release))
        // 50,000 Unicode characters can be 600 KB when percent-encoded.
        .layer(DefaultBodyLimit::max(768 * 1024))
        .with_state(state)
}

fn deny(code: StatusCode) -> Response {
    private_response(code, &[])
}

fn author_error(error: AuthorError) -> Response {
    deny(match error {
        AuthorError::Invalid => StatusCode::BAD_REQUEST,
        AuthorError::Forbidden => StatusCode::FORBIDDEN,
        AuthorError::Missing => StatusCode::NOT_FOUND,
        AuthorError::Conflict => StatusCode::CONFLICT,
        AuthorError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    })
}

async fn actor(state: &EditorState, headers: &HeaderMap) -> Result<Option<i64>, StatusCode> {
    let Some(token) = state.cookies.session_token(headers) else {
        return Ok(None);
    };
    state
        .accounts
        .active_session_user(&session_token_digest(&token))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}

async fn chapter_form(
    State(state): State<EditorState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let user_id = match actor(&state, &headers).await {
        Ok(Some(user_id)) => user_id,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let book = match state.authoring.owned_book(user_id, id).await {
        Ok(Some(book)) => book,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(error) => return author_error(error),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(
        author_pages::new_chapter(&book, &csrf, &generate_session_token()),
        &cookies,
    )
}

async fn create_chapter(
    State(state): State<EditorState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<ChapterForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let user_id = match actor(&state, &headers).await {
        Ok(Some(user_id)) => user_id,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let book_id = match u64::try_from(id) {
        Ok(book_id) if book_id > 0 => book_id,
        _ => return deny(StatusCode::NOT_FOUND),
    };
    let input = match save_chapter(SaveChapterRequest {
        book_id,
        chapter_id: None,
        number: form.number,
        expected_revision: 0,
        title: &form.title,
        body: &form.body,
        access: "free",
        price_amount: 0,
        command_key: &form.command_key,
    }) {
        Ok(input) => input,
        Err(error) => return author_error(error),
    };
    match state.authoring.create_free_chapter(user_id, &input).await {
        Ok(_) => private_response(Redirect::to(&format!("/me/books/{id}/release")), &[]),
        Err(error) => author_error(error),
    }
}

async fn release_form(
    State(state): State<EditorState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let user_id = match actor(&state, &headers).await {
        Ok(Some(user_id)) => user_id,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let book = match state.authoring.owned_book(user_id, id).await {
        Ok(Some(book)) => book,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(error) => return author_error(error),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(author_pages::release_book(&book, &csrf), &cookies)
}

async fn release(
    State(state): State<EditorState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<ReleaseForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let user_id = match actor(&state, &headers).await {
        Ok(Some(user_id)) => user_id,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    match state.authoring.release_book(user_id, id).await {
        Ok(()) => private_response(Redirect::to(&format!("/books/{id}")), &[]),
        Err(error) => author_error(error),
    }
}
