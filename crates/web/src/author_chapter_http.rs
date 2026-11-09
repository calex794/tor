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
use std::sync::Arc;
use tor_accounts::services::security::generate_session_token;
use tor_accounts::{repositories::PgAccounts, services::security::session_token_digest};
use tor_books::{
    repositories::{paid_storage::PgPaidStorage, postgres::PgAuthoring},
    services::authoring::{save_chapter, AuthorError, ChapterAccess, SaveChapterRequest},
};

use crate::auth_cookies::{private_response, CookiePolicy};

mod chapter_import;
mod new_chapter_page;
mod publisher;
mod schedule;
pub(crate) use publisher::{FreeChapterPublisher, PaidChapterPublisher};

#[derive(Clone)]
struct AuthorChapterState {
    accounts: PgAccounts,
    authoring: PgAuthoring,
    cookies: CookiePolicy,
    paid_author: Option<Arc<dyn PaidChapterPublisher>>,
    free_author: Option<Arc<dyn FreeChapterPublisher>>,
}

#[derive(Deserialize)]
struct ChapterForm {
    number: i32,
    auto_number: Option<String>,
    title: String,
    body: String,
    command_key: String,
    access: Option<String>,
    price_amount: Option<i64>,
    save_as_draft: Option<String>,
    is_delayed_publication: Option<String>,
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
struct ReleaseChapterForm {
    expected_revision: i64,
    version_id: i64,
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
struct PublishForm {
    csrf_token: Option<String>,
}

pub(crate) fn router(
    accounts: PgAccounts,
    cookies: CookiePolicy,
    paid_author: Option<Arc<dyn PaidChapterPublisher>>,
    free_author: Option<Arc<dyn FreeChapterPublisher>>,
) -> Router {
    let images = paid_author.as_ref().map(|publisher| {
        crate::author_chapter_images::router(accounts.clone(), cookies.clone(), publisher.clone())
    });
    let authoring = PgAuthoring::new(accounts.pool().clone());
    let state = AuthorChapterState {
        accounts,
        authoring,
        cookies,
        paid_author,
        free_author,
    };
    let app = Router::new()
        .route("/me/books/{id}/chapters/new", get(new_chapter))
        .route("/me/books/{id}/chapters/import", get(chapter_import::page))
        .route(
            "/me/books/{id}/chapters/import-preview",
            post(chapter_import::preview),
        )
        .route(
            "/me/books/{id}/chapters/import-confirm",
            post(chapter_import::confirm),
        )
        .route("/me/books/{id}/chapters", post(create_chapter))
        .route(
            "/me/books/{id}/chapters/{chapter_id}/edit",
            get(edit_chapter).post(update_chapter),
        )
        .route(
            "/me/books/{id}/chapters/{chapter_id}/release",
            get(schedule::confirm_release).post(publish_chapter),
        )
        .route(
            "/me/books/{id}/chapters/{chapter_id}/schedule",
            post(schedule::schedule_chapter),
        )
        .route(
            "/me/books/{id}/chapters/{chapter_id}/cancel-schedule",
            post(schedule::cancel_schedule),
        )
        .route("/me/books/{id}/publish", post(publish_book))
        .layer(DefaultBodyLimit::max(2 * 1024 * 1024))
        .with_state(state);
    if let Some(images) = images {
        app.merge(images)
    } else {
        app
    }
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
    // Keep the live session and actor-scoped book proof at the registered
    // HTTP handler boundary; RIL governance audits this exact route.
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
    new_chapter_page::render_authenticated(state, actor, id, headers).await
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
    // G requires identical LF canonicalization for B's content SHA and
    // the ciphertext AAD. Never permit the form to choose a KEK or S3 path.
    let body = form.body.replace("\r\n", "\n").replace('\r', "\n");
    // NEwBOO's public form uses is_delayed_publication. Preserve the former
    // TOR save_as_draft POST field for forms already opened in a browser.
    let delayed_publication = match (
        form.is_delayed_publication.as_deref(),
        form.save_as_draft.as_deref(),
    ) {
        (None, None) => false,
        (Some("1"), None) | (None, Some("1")) | (Some("1"), Some("1")) => true,
        _ => return deny(StatusCode::BAD_REQUEST),
    };
    // NEwBOO: the displayed number is advisory; B allocates the real number
    // under the owner book lock. Existing explicit-number POSTs remain valid.
    let number = match form.auto_number.as_deref() {
        None => form.number,
        Some("1") => 0,
        _ => return deny(StatusCode::BAD_REQUEST),
    };
    let access = form.access.as_deref().unwrap_or("free");
    let request = match save_chapter(SaveChapterRequest {
        book_id: id,
        chapter_id: None,
        number,
        expected_revision: 0,
        title: &form.title,
        body: &body,
        access,
        price_amount: form.price_amount.unwrap_or(0),
        command_key: &form.command_key,
    }) {
        Ok(request) => request,
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    if request.access == ChapterAccess::Paid {
        let Some(publisher) = &state.paid_author else {
            return private_response(
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Платная публикация пока не включена.",
                ),
                &[],
            );
        };
        let paid_result = if delayed_publication {
            publisher.stage_new(actor, &request).await
        } else {
            publisher.publish_new(actor, &request).await
        };
        return match paid_result {
            Ok(()) => {
                let target = if delayed_publication {
                    format!("/me/books/{id}")
                } else {
                    format!("/books/{id}")
                };
                private_response(Redirect::to(&target), &[])
            }
            Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
            Err(AuthorError::Conflict) => private_response(
                (
                    StatusCode::CONFLICT,
                    "Глава уже создаётся или изменена в другой вкладке. Повторите отправку этой же формы.",
                ),
                &[],
            ),
            Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
            Err(AuthorError::Forbidden) => deny(StatusCode::FORBIDDEN),
            Err(AuthorError::Unavailable) => private_response(
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Не удалось завершить защищённую публикацию. Вернитесь к форме и повторите отправку без изменения данных.",
                ),
                &[],
            ),
        };
    }
    // Saving to a published book releases the free chapter immediately.
    // The B transaction writes its immutable edition, reader pointer and
    // first-release fact together; a draft book remains hidden until release.
    let is_public = match state.authoring.owned_book_is_public(actor, id as i64).await {
        Ok(value) => value,
        Err(AuthorError::Missing) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if delayed_publication && !is_public {
        return deny(StatusCode::BAD_REQUEST);
    }
    // New fresh-install path: no free chapter Markdown is written to PG.
    // G must verify the actual S3 GET before B publishes an edition.
    if let Some(publisher) = state.free_author.as_ref() {
        if delayed_publication {
            return deny(StatusCode::SERVICE_UNAVAILABLE);
        }
        return match publisher.publish_new(actor, &request).await {
            Ok(()) => private_response(Redirect::to(&format!("/me/books/{id}")), &[]),
            Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
            Err(AuthorError::Conflict) => deny(StatusCode::CONFLICT),
            Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
            Err(AuthorError::Forbidden) => deny(StatusCode::FORBIDDEN),
            Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
        };
    }
    // A build with the S3-free feature must never fall back to text in PG.
    #[cfg(feature = "s3-free")]
    return deny(StatusCode::SERVICE_UNAVAILABLE);
    #[cfg(not(feature = "s3-free"))]
    {
    let outcome = if delayed_publication {
        state
            .authoring
            .create_pending_free_chapter(actor, &request)
            .await
    } else if is_public {
        state
            .authoring
            .create_published_free_chapter(actor, &request)
            .await
    } else {
        state.authoring.create_free_chapter(actor, &request).await
    };
    match outcome {
        Ok(saved) if delayed_publication => private_response(
            Redirect::to(&format!(
                "/me/books/{id}/chapters/{}/release",
                saved.chapter_id
            )),
            &[],
        ),
        Ok(_) => private_response(Redirect::to(&format!("/me/books/{id}")), &[]),
        Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Conflict) => deny(StatusCode::CONFLICT),
        Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(AuthorError::Forbidden) => deny(StatusCode::FORBIDDEN),
        Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
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
    // Published free content must be editable without a second release
    // action. Existing owner-only drafts still use their separate B path.
    let published = match state
        .authoring
        .owned_published_free_chapter(actor, id, chapter_id)
        .await
    {
        Ok(chapter) => chapter,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let paid = if published.is_none() && state.paid_author.is_some() {
        match PgPaidStorage::new(state.accounts.pool().clone())
            .owned_published_paid_chapter(actor, id, chapter_id)
            .await
        {
            Ok(paid) => paid,
            Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        }
    } else {
        None
    };
    let (chapter, is_published, is_paid) = if let Some(chapter) = published {
        (chapter, true, false)
    } else if let Some(chapter) = paid {
        (chapter, true, true)
    } else {
        match state
            .authoring
            .owned_unpublished_free_chapter(actor, id, chapter_id)
            .await
        {
            Ok(Some(chapter)) => (chapter, false, false),
            Ok(None) => return deny(StatusCode::NOT_FOUND),
            Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        }
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let command_key = generate_session_token();
    let page = if is_paid {
        crate::author_chapter_pages::edit_published_paid_chapter(
            id,
            chapter_id,
            &chapter,
            &csrf,
            &command_key,
        )
    } else if is_published {
        crate::author_chapter_pages::edit_published_chapter(
            id,
            chapter_id,
            &chapter,
            &csrf,
            &command_key,
        )
    } else {
        crate::author_chapter_pages::edit_chapter(id, chapter_id, &chapter, &csrf, &command_key)
    };
    private_response(page, &cookies)
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
    // B decides paid/publication state, never a submitted "paid" form field.
    // The owner query and the publishing transaction both check ownership.
    let paid = if state.paid_author.is_some() {
        match PgPaidStorage::new(state.accounts.pool().clone())
            .owned_published_paid_chapter(actor, id, chapter_id)
            .await
        {
            Ok(paid) => paid,
            Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        }
    } else {
        None
    };
    if let (Some(paid), Some(publisher)) = (paid, state.paid_author.as_ref()) {
        let body = form.body.replace("\r\n", "\n").replace('\r', "\n");
        let input = match save_chapter(SaveChapterRequest {
            book_id: book,
            chapter_id: Some(chapter),
            number: form.number,
            expected_revision: form.expected_revision,
            title: &form.title,
            body: &body,
            access: "paid",
            price_amount: paid.price_amount,
            command_key: &form.command_key,
        }) {
            Ok(input) => input,
            Err(_) => return deny(StatusCode::BAD_REQUEST),
        };
        return match publisher.publish_edit(actor, &input).await {
            Ok(()) => private_response(Redirect::to(&format!("/me/books/{id}/issues")), &[]),
            Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
            Err(AuthorError::Conflict) => private_response(
                (
                    StatusCode::CONFLICT,
                    "Глава уже изменена. Обновите страницу редактора.",
                ),
                &[],
            ),
            Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
            Err(AuthorError::Forbidden) => deny(StatusCode::FORBIDDEN),
            Err(AuthorError::Unavailable) => private_response(
                (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Защищённое сохранение не завершено. Повторите эту же форму.",
                ),
                &[],
            ),
        };
    }
    // Free-edition edits must not silently fall back to storing Markdown in PG.
    if cfg!(feature = "s3-free") {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    }
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
    // Route by trusted PG state, not by a browser-controlled "published"
    // field. B repeats ownership, state and expected-revision checks under
    // locks before any immutable edition or pointer can change.
    let published = match state
        .authoring
        .owned_published_free_chapter(actor, id, chapter_id)
        .await
    {
        Ok(Some(_)) => true,
        Ok(None) => false,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let outcome = if published {
        state
            .authoring
            .edit_published_free_chapter(actor, &input)
            .await
    } else {
        state
            .authoring
            .edit_unpublished_free_chapter(actor, &input)
            .await
    };
    match outcome {
        Ok(_) => private_response(Redirect::to(&format!("/me/books/{id}")), &[]),
        Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Conflict) => private_response(
            (
                StatusCode::CONFLICT,
                "Глава уже изменена в другой вкладке. Обновите страницу перед сохранением.",
            ),
            &[],
        ),
        Err(AuthorError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(AuthorError::Forbidden) => deny(StatusCode::FORBIDDEN),
        Err(AuthorError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

async fn publish_chapter(
    State(state): State<AuthorChapterState>,
    Path((id, chapter_id)): Path<(i64, i64)>,
    headers: HeaderMap,
    Form(form): Form<ReleaseChapterForm>,
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
    // S349 does not permit body_markdown-dependent pending releases.
    if cfg!(feature = "s3-free") {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    }
    match state
        .authoring
        .release_pending_free_chapter(
            actor,
            id,
            chapter_id,
            form.expected_revision,
            form.version_id,
        )
        .await
    {
        Ok(()) => private_response(Redirect::to(&format!("/books/{id}")), &[]),
        Err(AuthorError::Missing) => deny(StatusCode::NOT_FOUND),
        Err(AuthorError::Conflict) => private_response(
            (
                StatusCode::CONFLICT,
                "Глава изменилась после открытия подтверждения. Перезагрузите страницу.",
            ),
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
