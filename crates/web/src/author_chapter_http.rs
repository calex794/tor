//! Browser author publishing, only for authenticated owner, no URL collectors.
#![forbid(unsafe_code)]

use axum::{
    extract::{DefaultBodyLimit, Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use tor_accounts::services::security::generate_session_token;
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::authoring::{save_chapter, AuthorError, SaveChapterRequest},
};

use crate::auth_cookies::{private_response, CookiePolicy};

#[derive(Clone)]
struct AuthorChapterState {
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
struct EditChapterForm {
    number: i32,
    title: String,
    body: String,
    expected_revision: i64,
    command_key: String,
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
struct PublishForm {
    csrf_token: Option<String>,
}

pub(crate) fn router(accounts: PgAccounts, cookies: CookiePolicy) -> Router {
    let authoring = PgAuthoring::new(accounts.pool().clone());
    let state = AuthorChapterState {
        accounts,
        authoring,
        cookies,
    };
    Router::new()
        .route("/me/books/{id}/chapters/new", get(new_chapter))
        .route("/me/books/{id}/chapters", post(create_chapter))
        .route(
            "/me/books/{id}/chapters/{chapter_id}/edit",
            get(edit_chapter).post(update_chapter),
        )
        .route("/me/books/{id}/publish", post(publish_book))
        .layer(DefaultBodyLimit::max(768 * 1024))
        .with_state(state)
}

async fn session_actor(
    state: &AuthorChapterState,
    headers: &HeaderMap,
) -> Result<Option<i64>, StatusCode> {
    let Some(token) = state.cookies.session_token(headers) else {
        return Ok(None);
    };
    state
        .accounts
        .active_session_user(&session_token_digest(&token))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}

fn deny(status: StatusCode) -> Response {
    private_response(status, &[])
}

async fn new_chapter(
    State(state): State<AuthorChapterState>,
    Path(id): Path<i64>,
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
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let key = generate_session_token();
    private_response(
        crate::author_chapter_pages::new_chapter(id, &csrf, &key),
        &cookies,
    )
}

async fn create_chapter(
    State(state): State<AuthorChapterState>,
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
    let actor = match session_actor(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let id = match u64::try_from(id) {
        Ok(id) if id > 0 => id,
        _ => return deny(StatusCode::NOT_FOUND),
    };
    let request = match save_chapter(SaveChapterRequest {
        book_id: id,
        chapter_id: None,
        number: form.number,
        expected_revision: 0,
        title: &form.title,
        body: &form.body,
        access: "free",
        price_amount: 0,
        command_key: &form.command_key,
    }) {
        Ok(request) => request,
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    match state.authoring.create_free_chapter(actor, &request).await {
        Ok(_) => private_response(Redirect::to(&format!("/me/books/{id}")), &[]),
        Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Conflict) => deny(StatusCode::CONFLICT),
        Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(AuthorError::Forbidden) => deny(StatusCode::FORBIDDEN),
        Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

/// Prepopulate a draft editor only after resolving the active database session.
async fn edit_chapter(
    State(state): State<AuthorChapterState>,
    Path((id, chapter_id)): Path<(i64, i64)>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_actor(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let chapter = match state
        .authoring
        .owned_unpublished_free_chapter(actor, id, chapter_id)
        .await
    {
        Ok(Some(chapter)) => chapter,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(
        crate::author_chapter_pages::edit_chapter(
            id,
            chapter_id,
            &chapter,
            &csrf,
            &generate_session_token(),
        ),
        &cookies,
    )
}

/// Two tabs can open the same revision, but only one may save it.
async fn update_chapter(
    State(state): State<AuthorChapterState>,
    Path((id, chapter_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(form): Form<EditChapterForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_actor(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let book = match u64::try_from(id) {
        Ok(book) if book > 0 => book,
        _ => return deny(StatusCode::NOT_FOUND),
    };
    let chapter = match u64::try_from(chapter_id) {
        Ok(chapter) if chapter > 0 => chapter,
        _ => return deny(StatusCode::NOT_FOUND),
    };
    let input = match save_chapter(SaveChapterRequest {
        book_id: book,
        chapter_id: Some(chapter),
        number: form.number,
        expected_revision: form.expected_revision,
        title: &form.title,
        body: &form.body,
        access: "free",
        price_amount: 0,
        command_key: &form.command_key,
    }) {
        Ok(input) => input,
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    match state.authoring.edit_unpublished_free_chapter(actor, &input).await {
        Ok(_) => private_response(Redirect::to(&format!("/me/books/{id}")), &[]),
        Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Conflict) => private_response(
            (StatusCode::CONFLICT, "Глава изменена в другой вкладке или уже опубликована. Обновите страницу."),
            &[],
        ),
        Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(AuthorError::Forbidden) => deny(StatusCode::FORBIDDEN),
        Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

async fn publish_book(
    State(state): State<AuthorChapterState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<PublishForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_actor(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    match state.authoring.release_book(actor, id).await {
        Ok(()) => private_response(Redirect::to(&format!("/books/{id}")), &[]),
        Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Conflict) => deny(StatusCode::CONFLICT),
        Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(AuthorError::Forbidden) => deny(StatusCode::FORBIDDEN),
        Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}
