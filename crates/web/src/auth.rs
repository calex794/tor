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
    services::authoring::{
        new_book_with_taxonomy, with_book_description, AuthorError, TaxonomyFields,
    },
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
mod batch_checkout_browser;
mod behavior_consent;
mod book_comments;
mod book_description;
mod book_stars;
mod checkout;
mod complete_chapter;
mod layer_books;
mod layer_checkout;
mod layer_create;
mod layer_offer_manage;
mod library_actions;
mod login_limiter;
mod logout;
mod paid_completion;
mod rankings;
mod sync_actions;
mod types;
use types::*;
pub(crate) fn router(
    accounts: PgAccounts,
    reader: PgReader,
    secure: bool,
    origin: String,
) -> Router {
    router_inner(accounts, reader, secure, origin, None, false, None)
}

pub(crate) fn router_with_cover_uploads(
    accounts: PgAccounts,
    reader: PgReader,
    secure: bool,
    origin: String,
    storage: Arc<dyn CoverObjectStore>,
) -> Router {
    router_inner(accounts, reader, secure, origin, Some(storage), false, None)
}

/// Enable purchased-library navigation only when an authorized reader is mounted.
pub(crate) fn router_with_paid_reader(
    accounts: PgAccounts,
    reader: PgReader,
    secure: bool,
    origin: String,
    cover_upload_storage: Option<Arc<dyn CoverObjectStore>>,
) -> Router {
    router_inner(
        accounts,
        reader,
        secure,
        origin,
        cover_upload_storage,
        true,
        None,
    )
}

/// Only the private, explicitly enabled paid-author router provides a writer.
/// The existing grant-only constructor never changes author capabilities.
pub(crate) fn router_with_paid_reader_author(
    accounts: PgAccounts,
    reader: PgReader,
    secure: bool,
    origin: String,
    cover_upload_storage: Option<Arc<dyn CoverObjectStore>>,
    author: Arc<dyn author_chapter_http::PaidChapterPublisher>,
) -> Router {
    router_inner(
        accounts,
        reader,
        secure,
        origin,
        cover_upload_storage,
        true,
        Some(author),
    )
}

fn router_inner(
    accounts: PgAccounts,
    reader: PgReader,
    secure: bool,
    origin: String,
    storage: Option<Arc<dyn CoverObjectStore>>,
    paid_reader_available: bool,
    paid_author: Option<Arc<dyn author_chapter_http::PaidChapterPublisher>>,
) -> Router {
    let cookies = CookiePolicy { secure, origin };
    let chapter_routes =
        author_chapter_http::router(accounts.clone(), cookies.clone(), paid_author);
    let cover_routes = storage
        .as_ref()
        .map(|store| author_cover_http::router(accounts.clone(), cookies.clone(), store.clone()));
    let progress = PgReadingProgress::new(accounts.pool().clone());
    let sync = PgReadingSync::new(accounts.pool().clone());
    let bookmarks = PgLibrary::new(accounts.pool().clone());
    let rankings = tor_recommendations::repositories::PgRanking::new(accounts.pool().clone());
    let authoring = PgAuthoring::new(accounts.pool().clone());
    let checkout = tor_commerce::repositories::checkout::PgCheckout::new(accounts.pool().clone());
    let paid_checkout_enabled = std::env::var("TOR_PAID_CHECKOUT_ENABLED").as_deref() == Ok("1");
    let cover_jobs = storage
        .as_ref()
        .map(|_| PgCoverJobs::new(accounts.pool().clone()));
    let cover_upload_enabled = storage.is_some();
    let state = AuthState {
        accounts,
        checkout,
        paid_checkout_enabled,
        paid_reader_available,
        authoring,
        progress,
        sync,
        bookmarks,
        reader,
        rankings,
        cookies,
        password_jobs: PasswordJobs::new(4),
        ril_signer: behavior_consent::configured_ril_signer(),
        cover_jobs,
        cover_upload_enabled,
    };
    let observation_state = state.clone();
    let app = Router::new()
        .route(
            "/register",
            get(account_form_http::show_registration).post(account_form_http::register),
        )
        .route(
            "/login",
            get(account_form_http::show_login).post(account_form_http::login),
        )
        .route("/me", get(me))
        .route("/me/library", get(my_library))
        .route("/me/rankings", get(rankings::page))
        .route(
            "/me/books/{id}/rate",
            get(book_stars::page).post(book_stars::submit),
        )
        .route(
            "/books/{id}/comments",
            get(book_comments::page).post(book_comments::submit),
        )
        .route("/me/rankings/choice", post(rankings::browser_choice))
        .route("/me/rankings/resume", post(rankings::browser_resume))
        .route("/api/v1/me/rankings", get(rankings::api_state))
        .route("/api/v1/me/rankings/choice", post(rankings::api_choice))
        .route("/api/v1/me/rankings/resume", post(rankings::api_resume))
        .route("/api/v1/purchases", post(checkout::buy))
        .route("/api/v1/purchases/batch", post(checkout::buy_batch))
        .route(
            "/me/books/{id}/buy-chapters",
            get(batch_checkout_browser::page).post(batch_checkout_browser::submit),
        )
        .route(
            "/api/v1/me/progress/free",
            get(sync_actions::get_free).put(sync_actions::put_free),
        )
        .route(
            "/api/v1/me/progress/purchased",
            get(sync_actions::get_purchased).put(sync_actions::put_purchased),
        )
        .route(
            "/api/v1/me/progress/purchased/complete",
            get(paid_completion::get).post(paid_completion::post),
        )
        .route("/me/library/add/{id}", get(library_actions::show_add))
        .route("/me/library/{id}/save", post(library_actions::browser_add))
        .route(
            "/me/library/{id}/remove",
            post(library_actions::browser_remove),
        )
        .route("/api/v1/me/library", get(library_actions::api_list))
        .route(
            "/api/v1/me/library/{id}",
            post(library_actions::api_add).delete(library_actions::api_delete),
        )
        .route("/me/books", get(author_books).post(create_book))
        .route("/me/books/new", get(new_author_book))
        .route("/me/books/{id}", get(author_book))
        .route(
            "/me/books/{id}/description",
            get(book_description::edit).post(book_description::save),
        )
        .route("/me/books/{id}/settings/covers", get(cover_settings))
        .route("/read/{id}", get(private_reader))
        .route("/read/{id}/place", post(save_place))
        .route("/read/{id}/complete", post(complete_chapter::complete))
        .route("/logout", post(logout::logout))
        .route("/me/behavior", post(behavior_consent::set_behavior_consent))
        .merge(layer_books::routes())
        .merge(layer_create::routes())
        .merge(layer_checkout::routes())
        .merge(layer_offer_manage::routes())
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(state)
        .layer(middleware::from_fn_with_state(
            observation_state,
            behavior_consent::annotate_verified_ril,
        ))
        .merge(chapter_routes);
    if let Some(cover_routes) = cover_routes {
        app.merge(cover_routes)
    } else {
        app
    }
}
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

/// Browser author workspace backed by the current PostgreSQL user session.
async fn author_books(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let books = match state.authoring.list_owned_books(actor).await {
        Ok(books) => books,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if !layer_books::enabled() {
        return private_response(author_pages::index(&books), &[]);
    }
    let actor = match tor_contracts::paid_delivery::PositiveId::new(actor) {
        Ok(actor) => actor,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let layers = tor_commerce::services::layers::LayerService::new(
        tor_commerce::repositories::layers::PgLayers::new(state.accounts.pool().clone()),
    );
    match layers.owned_layers(actor, 100).await {
        Ok(layers) => private_response(author_pages::index_with_layers(&books, &layers), &[]),
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
    )
    .and_then(|book| with_book_description(book, form.description.as_deref().unwrap_or("")))
    {
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
