//! Real browser account flows, backed by PostgreSQL and revocable sessions.
#![forbid(unsafe_code)]

use crate::{
    author_pages,
    auth_cookies::{private_response, CookiePolicy, SESSION_AGE},
    pages,
    password_jobs::PasswordJobs,
};
use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::{net::SocketAddr, num::NonZeroU32};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{
        generate_session_token, hash_password, session_token_digest, verify_password,
    },
    services::validation as rules,
};
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::authoring::{new_book, AuthorError},
};
use tor_reading::repositories::PgReadingProgress;
use tor_reading::{
    repositories::PgReader,
    services::{
        progress::{reader_blocks, valid_text_place, ProgressError, ReadingPlace},
        PublicReadingRepository,
    },
};

#[derive(Clone)]
struct AuthState {
    accounts: PgAccounts,
    authoring: PgAuthoring,
    progress: PgReadingProgress,
    reader: PgReader,
    cookies: CookiePolicy,
    password_jobs: PasswordJobs,
}

#[derive(Deserialize)]
struct RegisterForm {
    email: String,
    display_name: String,
    password: String,
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
struct LoginForm {
    email: String,
    password: String,
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
struct LogoutForm {
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
struct SavePlaceForm {
    csrf_token: Option<String>,
    version_id: u64,
    block_index: i32,
    char_offset: i32,
}

#[derive(Deserialize)]
struct NewBookForm {
    title: String,
    target_language: String,
    command_key: String,
    csrf_token: Option<String>,
}

pub(crate) fn router(
    accounts: PgAccounts,
    reader: PgReader,
    secure: bool,
    origin: String,
) -> Router {
    let progress = PgReadingProgress::new(accounts.pool().clone());
    let authoring = PgAuthoring::new(accounts.pool().clone());
    let state = AuthState {
        accounts,
        authoring,
        progress,
        reader,
        cookies: CookiePolicy { secure, origin },
        password_jobs: PasswordJobs::new(4),
    };
    Router::new()
        .route("/register", get(show_registration).post(register))
        .route("/login", get(show_login).post(login))
        .route("/me", get(me))
        .route("/me/library", get(my_library))
        .route("/me/books", get(author_books).post(create_book))
        .route("/me/books/new", get(new_author_book))
        .route("/me/books/{id}", get(author_book))
        .route("/read/{id}", get(private_reader))
        .route("/read/{id}/place", post(save_place))
        .route("/logout", post(logout))
        .layer(DefaultBodyLimit::max(4096))
        .with_state(state)
}

async fn show_registration(State(state): State<AuthState>) -> Response {
    let (token, cookie) = state.cookies.csrf_token_cookie();
    private_response(pages::registration_form(&token), &[cookie])
}

async fn show_login(State(state): State<AuthState>) -> Response {
    let (token, cookie) = state.cookies.csrf_token_cookie();
    private_response(pages::login_form(&token), &[cookie])
}

fn deny(status: StatusCode) -> Response {
    private_response(status, &[])
}

fn invalid_credentials() -> Response {
    private_response(
        (StatusCode::UNAUTHORIZED, "Неверный адрес почты или пароль"),
        &[],
    )
}

/// Reserve capacity before Argon2 to slow password guessing and account spray.
/// A client IP comes from the TCP peer, NEVER from untrusted forwarding headers.
async fn limit(
    state: &AuthState,
    action: &str,
    email: &str,
    peer: ConnectInfo<SocketAddr>,
) -> Result<[u8; 32], StatusCode> {
    let identity_key = session_token_digest(&format!("auth:{action}:identity:{email}"));
    let ip = peer.0.ip().to_string();
    let address_key = session_token_digest(&format!("auth:{action}:peer:{ip}"));
    // No in-process counters: these limits apply across all server processes.
    let address_ok = state
        .accounts
        .permit_auth_attempt(&address_key, 60, 600)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if !address_ok {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    let identity_ok = state
        .accounts
        .permit_auth_attempt(&identity_key, 8, 600)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if !identity_ok {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    Ok(identity_key)
}

async fn register(
    State(state): State<AuthState>,
    headers: HeaderMap,
    peer: ConnectInfo<SocketAddr>,
    Form(form): Form<RegisterForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let account = match rules::registration(&form.email, &form.display_name, &form.password) {
        Ok(account) => account,
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    if let Err(status) = limit(&state, "register", &account.email, peer).await {
        return deny(status);
    }

    // Argon2 is deliberately moved off Tokio's request executor.
    let password = form.password;
    let hashed = match state
        .password_jobs
        .run(move || hash_password(&password))
        .await
    {
        Ok(Ok(hash)) => hash,
        Ok(Err(_)) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        Err(status) => return deny(status),
    };
    let token = generate_session_token();
    let digest = session_token_digest(&token);
    let ttl = NonZeroU32::new(SESSION_AGE).expect("nonzero session lifetime");
    match state
        .accounts
        .register_with_session(&account.email, &account.display_name, &hashed, &digest, ttl)
        .await
    {
        Ok(_) => private_response(Redirect::to("/me"), &[state.cookies.session_cookie(&token)]),
        Err(tor_accounts::repositories::postgres::AccountWriteError::Duplicate) => {
            // No user ID or database exception is disclosed.
            private_response(
                (StatusCode::CONFLICT, "Не удалось создать учётную запись"),
                &[],
            )
        }
        Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

async fn login(
    State(state): State<AuthState>,
    headers: HeaderMap,
    peer: ConnectInfo<SocketAddr>,
    Form(form): Form<LoginForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let email = match rules::login(&form.email, &form.password) {
        Ok(email) => email,
        Err(_) => return invalid_credentials(),
    };
    let identity_key = match limit(&state, "login", &email, peer).await {
        Ok(key) => key,
        Err(status) => return deny(status),
    };
    let stored = match state.accounts.find_user(&email).await {
        Ok(stored) => stored,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let known_hash = stored.as_ref().map(|u| u.password_hash.clone());
    let password = form.password;
    let verified = match state
        .password_jobs
        .run(move || match known_hash {
            Some(hash) => verify_password(&password, &hash),
            None => {
                // A missing account still performs costly work to limit timing leaks.
                let _ = hash_password(&password);
                false
            }
        })
        .await
    {
        Ok(valid) => valid,
        Err(status) => return deny(status),
    };
    if !verified {
        return invalid_credentials();
    }
    let Some(user) = stored else {
        return invalid_credentials();
    };

    if state
        .accounts
        .clear_auth_attempt(&identity_key)
        .await
        .is_err()
    {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    }
    let token = generate_session_token();
    let digest = session_token_digest(&token);
    let ttl = NonZeroU32::new(SESSION_AGE).expect("nonzero session lifetime");
    if state
        .accounts
        .insert_session(user.id, &digest, ttl)
        .await
        .is_err()
    {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    }
    // No next parameter is accepted: redirects always stay on this site.
    private_response(Redirect::to("/me"), &[state.cookies.session_cookie(&token)])
}

async fn me(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let Some(token) = state.cookies.session_token(&headers) else {
        return private_response(Redirect::to("/login"), &[]);
    };
    let digest = session_token_digest(&token);
    let user_id = match state.accounts.active_session_user(&digest).await {
        Ok(Some(id)) => id,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let name = match state.accounts.user_display_name(user_id).await {
        Ok(Some(value)) => value,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(pages::my_account(&name, &csrf), &cookies)
}

async fn my_library(State(state): State<AuthState>, headers: HeaderMap) -> Response {
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
    private_response(pages::my_library(&books), &[])
}

/// Resolve the authenticated actor exclusively from the database-backed session.
/// A client-supplied user ID is never accepted.
async fn session_user(state: &AuthState, headers: &HeaderMap) -> Result<Option<i64>, StatusCode> {
    let Some(token) = state.cookies.session_token(headers) else {
        return Ok(None);
    };
    state
        .accounts
        .active_session_user(&session_token_digest(&token))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}

/// Browser author workspace backed by the current PostgreSQL user session.
async fn author_books(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    match state.authoring.list_owned_books(actor).await {
        Ok(books) => private_response(author_pages::index(&books), &[]),
        Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

async fn new_author_book(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    match session_user(&state, &headers).await {
        Ok(Some(_)) => (),
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    }
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    // Each form gets a unique cryptographic command key for safe POST replay.
    let command_key = generate_session_token();
    private_response(author_pages::new_book(&csrf, &command_key), &cookies)
}

async fn create_book(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<NewBookForm>,
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
    // Manual authorship does not invoke collectors or trust client-supplied IDs.
    // The source identity is stable across retries of this exact form.
    let identity = format!("manual:{actor}:{}", form.command_key);
    let request = match new_book(
        &identity,
        &form.target_language,
        &form.title,
        &form.command_key,
    ) {
        Ok(request) => request,
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    match state.authoring.create_owned_book(actor, &request).await {
        Ok(id) => private_response(Redirect::to(&format!("/me/books/{id}")), &[]),
        Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(AuthorError::Forbidden) => deny(StatusCode::FORBIDDEN),
        Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Conflict) => deny(StatusCode::CONFLICT),
        Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

async fn author_book(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    match state.authoring.owned_book(actor, id).await {
        Ok(Some(book)) => private_response(author_pages::detail(&book), &[]),
        Ok(None) => deny(StatusCode::NOT_FOUND),
        Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

/// Interactive reading is private to a verified account. Public free reading
/// continues to be served separately at /reader/{id}.
async fn private_reader(
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
        Ok(Some(chapter)) => chapter,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (chapter_id, version_id) = match (i64::try_from(id), i64::try_from(chapter.version_id)) {
        (Ok(chapter_id), Ok(version_id)) => (chapter_id, version_id),
        _ => return deny(StatusCode::NOT_FOUND),
    };
    let saved = match state
        .progress
        .saved_place(actor, chapter_id, version_id)
        .await
    {
        Ok(saved) => saved,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(pages::private_reader(id, &chapter, &csrf, saved), &cookies)
}

/// Manual, no-JavaScript place saving. Every write verifies session, Origin,
/// double-submit CSRF, current published version, and free-content visibility.
async fn save_place(
    State(state): State<AuthState>,
    Path(id): Path<u64>,
    headers: HeaderMap,
    Form(form): Form<SavePlaceForm>,
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
    let chapter = match state.reader.public_chapter(id).await {
        Ok(Some(chapter)) => chapter,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if chapter.version_id != form.version_id {
        return deny(StatusCode::CONFLICT);
    }
    let place = ReadingPlace {
        block_index: form.block_index,
        char_offset: form.char_offset,
    };
    let blocks = reader_blocks(&chapter.body_markdown);
    if valid_text_place(place, &blocks).is_err() {
        return deny(StatusCode::BAD_REQUEST);
    }
    let (chapter_id, version_id) = match (i64::try_from(id), i64::try_from(form.version_id)) {
        (Ok(chapter_id), Ok(version_id)) => (chapter_id, version_id),
        _ => return deny(StatusCode::NOT_FOUND),
    };
    match state
        .progress
        .save_position(actor, chapter_id, version_id, place)
        .await
    {
        Ok(()) => private_response(
            Redirect::to(&format!("/read/{id}#p-{}", place.block_index)),
            &[],
        ),
        Err(ProgressError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(ProgressError::NotFound) => deny(StatusCode::NOT_FOUND),
        Err(ProgressError::StaleVersion) => deny(StatusCode::CONFLICT),
        Err(ProgressError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

async fn logout(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<LogoutForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    if let Some(token) = state.cookies.session_token(&headers) {
        let digest = session_token_digest(&token);
        if state.accounts.revoke_session(&digest).await.is_err() {
            return deny(StatusCode::SERVICE_UNAVAILABLE);
        }
    }
    private_response(Redirect::to("/login"), &state.cookies.cleared_cookies())
}
