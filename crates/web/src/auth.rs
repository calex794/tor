#![forbid(unsafe_code)]

use crate::{
    auth_cookies::{deny, private_response, CookiePolicy},
    author_chapter_http, author_chapter_pages, author_cover_http, author_pages, pages,
    password_jobs::PasswordJobs,
};
use axum::{
    extract::{DefaultBodyLimit, Form, Path, State},
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
use tor_reading::repositories::{
    PgContinuationDemand, PgFollowUpdates, PgLibrary, PgNotificationPreferences, PgReadingProgress,
    PgReadingSync,
};
use tor_reading::{
    repositories::PgReader,
    services::{
        progress::{reader_blocks, valid_text_place, ProgressError, ReadingPlace},
        PublicReadingRepository,
    },
};
mod access_audit;
mod admin_mail;
mod admin_mail_routes;
mod auth_observation;
pub(crate) fn auth_observation_dropped() -> u64 {
    auth_observation::dropped_count()
}
mod account_form_http;
mod author_book_page;
mod batch_checkout_browser;
mod behavior_consent;
mod book_comments;
mod book_description;
mod book_follow_actions;
mod book_likes;
mod book_rooms;
mod book_stars;
mod book_stats_http;
mod chapter_thanks;
mod checkout;
mod complete_chapter;
mod continuation_demand;
mod creator_follows;
mod follow_read_state;
mod followed_purchase_links;
mod friendship;
mod group_mail;
mod inbox;
mod library_actions;
mod library_page;
mod login_limiter;
mod mail;
mod mail_api;
mod mail_book_previews;
mod mail_pages;
mod notification_settings;
mod notifications;
mod paid_completion;
mod password_change;
mod password_reset;
mod private_reader;
mod profile_http;
mod public_profile;
mod rankings;
mod referral_alias;
mod referral_capture;
mod referral_dashboard;
mod referral_routes;
mod reply_read_state;
mod reset_delivery;
mod security_retention;
mod session_security;
mod settings_routes;
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
    security_retention::start(accounts.clone());
    let reset_delivery = reset_delivery::ResetDelivery::from_env(&origin);
    let cookies = CookiePolicy { secure, origin };
    let pool = accounts.pool().clone();
    let rec_runtime = crate::recommendation_http::Runtime::new(accounts.clone(), cookies.clone());
    let chapter_routes =
        author_chapter_http::router(accounts.clone(), cookies.clone(), paid_author);
    let cover_routes = storage
        .as_ref()
        .map(|store| author_cover_http::router(accounts.clone(), cookies.clone(), store.clone()));
    let progress = PgReadingProgress::new(accounts.pool().clone());
    let sync = PgReadingSync::new(accounts.pool().clone());
    let bookmarks = PgLibrary::new(accounts.pool().clone());
    let demand = PgContinuationDemand::new(accounts.pool().clone());
    let book_follows = tor_reading::repositories::PgBookFollows::new(accounts.pool().clone());
    let creator_follows = tor_social::services::follow::PgCreatorFollows::new(pool.clone());
    let follow_updates = PgFollowUpdates::new(accounts.pool().clone());
    let rankings = tor_recommendations::repositories::PgRanking::new(accounts.pool().clone());
    let achievements = tor_recommendations::repositories::PgAchievements::new(pool.clone());
    let authoring = PgAuthoring::new(accounts.pool().clone());
    let checkout = tor_commerce::repositories::checkout::PgCheckout::new(accounts.pool().clone());
    let paid_checkout_enabled = std::env::var("TOR_PAID_CHECKOUT_ENABLED").as_deref() == Ok("1");
    let cover_jobs = storage.as_ref().map(|_| PgCoverJobs::new(pool.clone()));
    let cover_upload_enabled = storage.is_some();
    let state = AuthState {
        referrals: tor_referrals::repositories::PgReferrals::new(accounts.pool().clone()),
        reset_delivery,
        notification_preferences: PgNotificationPreferences::new(pool.clone()),
        accounts,
        checkout,
        paid_checkout_enabled,
        paid_reader_available,
        authoring,
        progress,
        sync,
        bookmarks,
        demand,
        book_follows,
        creator_follows,
        follow_updates,
        reader,
        rankings,
        achievements,
        cookies,
        password_jobs: PasswordJobs::new(4),
        ril_signer: behavior_consent::configured_ril_signer(),
        cover_jobs,
        cover_upload_enabled,
    };
    let observation_state = state.clone();
    let app = Router::new()
        .merge(settings_routes::router())
        .merge(referral_routes::router())
        .merge(notifications::router().merge(group_mail::invitations::router()))
        .route(
            "/register",
            get(account_form_http::show_registration).post(account_form_http::register),
        )
        .route(
            "/login",
            get(account_form_http::show_login).post(account_form_http::login),
        )
        .route("/me", get(me))
        .route("/me/creators", get(creator_follows::page))
        .route(
            "/me/creators/{id}/unfollow",
            post(creator_follows::unfollow),
        )
        .route(
            "/me/books/{id}/follow-creator",
            post(book_follow_actions::follow_creator),
        )
        .route("/users/{display_name}", get(public_profile::page))
        .route("/users/{target_user_id}/friend", post(friendship::request))
        .route("/users/{target_user_id}/unfriend", post(friendship::remove))
        .route("/users/{target_user_id}/block", post(friendship::block))
        .merge(admin_mail_routes::router())
        .route("/mail", get(mail::index))
        .route("/mail/start/{recipient_user_id}", post(mail::start))
        .route("/mail/{id}", get(mail::conversation))
        .route("/mail/{id}/messages", get(mail::poll).post(mail::send))
        .route("/mail/{id}/block", post(mail::block))
        .route(
            "/mail/groups",
            get(group_mail::index).post(group_mail::create),
        )
        .route("/mail/groups/{id}", get(group_mail::detail))
        .route(
            "/mail/groups/{id}/messages",
            get(group_mail::poll).post(group_mail::send),
        )
        .route("/mail/groups/{id}/leave", post(group_mail::leave))
        .route("/mail/groups/{id}/transfer", post(group_mail::transfer))
        .route("/mail/groups/{id}/disband", post(group_mail::disband))
        .route("/books/{id}/room", get(book_rooms::page))
        .route("/books/{id}/room/join", post(book_rooms::join))
        .route("/books/{id}/room/messages", post(book_rooms::send))
        .route("/books/{id}/room/leave", post(book_rooms::leave))
        .route(
            "/api/v1/me/messages",
            get(mail_api::get).post(mail_api::post),
        )
        .route("/assets/mail.js", get(mail::script))
        .route(
            "/me/settings/password",
            get(password_change::page).post(password_change::submit),
        )
        .route("/assets/mentions.js", get(book_comments::mention_script))
        .route(
            "/api/v1/social/mentions/discover",
            get(book_comments::discover_mentions),
        )
        .route("/me/inbox", get(inbox::page))
        .route("/api/v1/me/inbox", get(inbox::api))
        .route(
            "/me/replies/{id}/read-state",
            post(reply_read_state::submit),
        )
        .route("/me/library", get(library_page::my_library))
        .route(
            "/me/updates/{id}/read-state",
            post(follow_read_state::submit),
        )
        .route("/me/rankings", get(rankings::page))
        .route(
            "/books/{id}/like",
            get(book_likes::page).post(book_likes::submit),
        )
        .route(
            "/me/books/{id}/rate",
            get(book_stars::page).post(book_stars::submit),
        )
        .route(
            "/reader/{id}/thanks",
            get(chapter_thanks::page).post(chapter_thanks::submit),
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
        .route("/me/books/{id}/follow", post(book_follow_actions::follow))
        .route(
            "/me/books/{id}/unfollow",
            post(book_follow_actions::unfollow),
        )
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
        .route("/me/books/{id}", get(author_book_page::page))
        .route("/me/books/{id}/preview", get(mail_book_previews::page))
        .route("/books/{id}/stats", get(book_stats_http::page))
        .route("/books/{id}/stats/export.csv", get(book_stats_http::export))
        .route(
            "/me/books/{id}/description",
            get(book_description::edit).post(book_description::save),
        )
        .route("/me/books/{id}/settings/covers", get(cover_settings))
        .route("/read/{id}", get(private_reader::page))
        .route("/read/{id}/place", post(save_place))
        .route("/read/{id}/complete", post(complete_chapter::complete))
        .route(
            "/read/{id}/want-continuation",
            post(continuation_demand::submit),
        )
        .route("/logout", post(logout))
        .route("/me/behavior", post(behavior_consent::set_behavior_consent))
        .layer(DefaultBodyLimit::max(64 * 1024))
        .with_state(state)
        .layer(middleware::from_fn_with_state(
            observation_state,
            behavior_consent::annotate_verified_ril,
        ))
        .merge(chapter_routes);
    let app = cover_routes
        .into_iter()
        .fold(app, |app, cover| app.merge(cover));
    let app = app.merge(crate::recommendation_http::routes(rec_runtime.clone()));
    crate::recommendation_http::instrument(app, Some(rec_runtime))
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
    let discussions =
        tor_discussions::repositories::public_book_comments::PgPublicBookComments::new(
            state.accounts.pool().clone(),
        );
    let replies = match discussions.received_replies(user_id, 10).await {
        Ok(replies) => replies,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let unread = match state.follow_updates.unread_count_capped(user_id).await {
        Ok(count) => count,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let achievements = match state.achievements.snapshot(user_id).await {
        Ok(achievements) => achievements,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(
        pages::my_account_with_achievements(&name, &csrf, &replies, unread, &achievements),
        &cookies,
    )
}
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
