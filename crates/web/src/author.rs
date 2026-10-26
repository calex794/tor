//! Private book-creation routes; all writes go through tor-books and PostgreSQL.
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
    services::authoring::{new_book, AuthorError},
};

#[derive(Clone)]
struct AuthorState {
    accounts: PgAccounts,
    authoring: PgAuthoring,
    cookies: CookiePolicy,
}

#[derive(Deserialize)]
struct CreateBookForm {
    source_identity: String,
    target_language: String,
    title: String,
    command_key: String,
    csrf_token: Option<String>,
}

pub(crate) fn router(accounts: PgAccounts, secure: bool, origin: String) -> Router {
    let authoring = PgAuthoring::new(accounts.pool().clone());
    let state = AuthorState {
        accounts,
        authoring,
        cookies: CookiePolicy { secure, origin },
    };
    Router::new()
        .route("/books/new", get(new_book_form).post(create_book))
        .route("/me/books", get(my_books))
        .route("/author/books/{id}", get(owned_book))
        .layer(DefaultBodyLimit::max(4096))
        .with_state(state)
}

fn deny(status: StatusCode) -> Response {
    private_response(status, &[])
}

async fn actor(state: &AuthorState, headers: &HeaderMap) -> Result<Option<i64>, StatusCode> {
    let Some(token) = state.cookies.session_token(headers) else {
        return Ok(None);
    };
    state
        .accounts
        .active_session_user(&session_token_digest(&token))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}

async fn new_book_form(State(state): State<AuthorState>, headers: HeaderMap) -> Response {
    match actor(&state, &headers).await {
        Ok(Some(_)) => {}
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let key = generate_session_token();
    private_response(author_pages::new_book(&csrf, &key), &cookies)
}

async fn create_book(
    State(state): State<AuthorState>,
    headers: HeaderMap,
    Form(form): Form<CreateBookForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let user_id = match actor(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let command = match new_book(
        &form.source_identity,
        &form.target_language,
        &form.title,
        &form.command_key,
    ) {
        Ok(command) => command,
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    match state.authoring.create_owned_book(user_id, &command).await {
        Ok(id) => private_response(Redirect::to(&format!("/author/books/{id}")), &[]),
        Err(AuthorError::Conflict) => deny(StatusCode::CONFLICT),
        Err(AuthorError::Forbidden) => deny(StatusCode::FORBIDDEN),
        Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

async fn my_books(State(state): State<AuthorState>, headers: HeaderMap) -> Response {
    let user_id = match actor(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    match state.authoring.list_owned_books(user_id).await {
        Ok(books) => private_response(author_pages::my_books(&books), &[]),
        Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

async fn owned_book(
    State(state): State<AuthorState>,
    Path(id): Path<u64>,
    headers: HeaderMap,
) -> Response {
    let user_id = match actor(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let book_id = match i64::try_from(id) {
        Ok(id) if id > 0 => id,
        _ => return deny(StatusCode::NOT_FOUND),
    };
    match state.authoring.owned_book(user_id, book_id).await {
        Ok(Some(book)) => private_response(author_pages::owned_book(&book), &[]),
        Ok(None) => deny(StatusCode::NOT_FOUND),
        Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}
