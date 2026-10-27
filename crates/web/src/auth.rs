//! Real browser account flows, backed by PostgreSQL and revocable sessions.
#![forbid(unsafe_code)]

use crate::{
    auth_cookies::{private_response, CookiePolicy},
    author_chapter_http, author_chapter_pages, author_cover_http, author_pages, pages,
    password_jobs::PasswordJobs,
};
use axum::{
    extract::{DefaultBodyLimit, Form, Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    middleware,
    response::{Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::sync::Arc;
use tor_accounts::{
    repositories::PgAccounts,
    services::ril_pseudonym::RilSubjectSigner,
    services::security::{generate_session_token, session_token_digest},
};
use tor_books::{
    repositories::postgres::PgAuthoring,
    services::authoring::{new_book_with_taxonomy, AuthorError, TaxonomyFields},
};
use tor_ingest::{repositories::cover_jobs::PgCoverJobs, services::cover_jobs::CoverObjectStore};
use tor_reading::domain::renderer::valid_reader_length;
use tor_reading::repositories::{PgLibrary, PgReadingProgress, PgReadingSync};
use tor_reading::{
    repositories::PgReader,
    services::{
        progress::{reader_blocks, valid_text_place, ProgressError, ReadingPlace},
        PublicReadingRepository,
    },
};

mod account_form_http;
mod behavior_consent;
mod checkout;
mod complete_chapter;
mod library_actions;
mod login_limiter;
mod paid_completion;
mod rankings;
mod sync_actions;

#[derive(Clone)]
struct AuthState {
    accounts: PgAccounts,
    checkout: tor_commerce::repositories::checkout::PgCheckout,
    paid_checkout_enabled: bool,
    paid_reader_available: bool,
    authoring: PgAuthoring,
    progress: PgReadingProgress,
    sync: PgReadingSync,
    bookmarks: PgLibrary,
    reader: PgReader,
    rankings: tor_recommendations::repositories::PgRanking,
    cookies: CookiePolicy,
    password_jobs: PasswordJobs,
    ril_signer: Option<RilSubjectSigner>,
    cover_jobs: Option<PgCoverJobs>,
    cover_upload_enabled: bool,
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

#[derive(Default, Deserialize)]
struct LibraryQuery {
    after: Option<i64>,
    limit: Option<u16>,
}

#[derive(Deserialize)]
struct NewBookForm {
    title: String,
    target_language: String,
    author_name: Option<String>,
    genres: Option<String>,
    tags: Option<String>,
    fandoms: Option<String>,
    command_key: String,
    csrf_token: Option<String>,
}

mod router_setup;
pub(crate) use router_setup::{router, router_with_cover_uploads, router_with_paid_reader, router_with_paid_reader_author};

fn deny(status: StatusCode) -> Response {
    private_response(status, &[])
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

async fn my_library(
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
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let page = if state.paid_reader_available {
        pages::my_library_with_purchases(&books, &saved, &csrf, true)
    } else {
        pages::my_library(&books, &saved, &csrf)
    };
    private_response(page, &cookies)
}

/// Resolve actor from the session database, never client-supplied user IDs.
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
    let request = match new_book_with_taxonomy(
        &identity,
        &form.target_language,
        &form.title,
        &form.command_key,
        form.author_name.as_deref().unwrap_or(""),
        form.genres.as_deref().unwrap_or(""),
        TaxonomyFields {
            tags: form.tags.as_deref().unwrap_or(""),
            fandoms: form.fandoms.as_deref().unwrap_or(""),
        },
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
    let book = match state.authoring.owned_book(actor, id).await {
        Ok(Some(book)) => book,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let published = match state.authoring.owned_book_is_public(actor, id).await {
        Ok(published) => published,
        Err(AuthorError::Missing) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let chapters = match state.authoring.list_owned_chapters(actor, id).await {
        Ok(chapters) => chapters,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let latest_cover = if let Some(jobs) = state.cover_jobs.as_ref() {
        match jobs.latest_for_author(id, actor).await {
            Ok(latest) => latest,
            Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        }
    } else {
        None
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(
        author_chapter_pages::detail_with_covers(
            &book,
            published,
            &chapters,
            &csrf,
            state.cover_upload_enabled,
            latest_cover.as_deref(),
        ),
        &cookies,
    )
}

/// NEwBOO owner cover settings, restricted script; no inline scripts.
async fn cover_settings(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    if !state.cover_upload_enabled {
        return deny(StatusCode::NOT_FOUND);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let book = match state.authoring.owned_book(actor, id).await {
        Ok(Some(book)) => book,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let latest = match state.cover_jobs.as_ref() {
        Some(jobs) => match jobs.latest_for_author(id, actor).await {
            Ok(latest) => latest,
            Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        },
        None => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let slots =
        tor_books::repositories::cover_slots::PgCoverSlots::new(state.accounts.pool().clone());
    let current = match slots
        .public_variant(
            id,
            tor_books::repositories::cover_slots::PublicCoverKind::Main,
        )
        .await
    {
        Ok(current) => current.is_some(),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let mut response = private_response(
        author_chapter_pages::cover_settings(&book, &csrf, latest.as_deref(), current),
        &cookies,
    );
    response.headers_mut().insert(
        "content-security-policy",
        HeaderValue::from_static(
            "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' blob:; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    response
}

/// Private verified reader; public free chapters use /reader/{id}.
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
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(
        pages::private_reader(id, &chapter, &csrf, saved, revision, completed),
        &cookies,
    )
}

/// Manual place save checks session, Origin, CSRF, published version and visibility.
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
        Ok(Some(chapter)) if valid_reader_length(&chapter.body_markdown) => chapter,
        Ok(Some(_)) => return deny(StatusCode::PAYLOAD_TOO_LARGE),
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
