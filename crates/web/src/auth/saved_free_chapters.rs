//! Authenticated, read-only private saved-free chapter navigation.
//! No public reader entitlement is inferred from a Book bookmark alone.
use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tor_reading::{
    domain::renderer::valid_reader_length,
    services::library::{
        LibraryError, SavedArchivePage, SavedArchiveTocCursor, SavedArchiveTocSide,
    },
};

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/me/library/{id}/saved", get(list))
        .route("/me/library/saved/{id}", get(read))
        .route("/api/v1/me/library/saved/{id}/toc/window", get(toc_window))
        .route("/assets/newboo-archive-progress.js", get(progress_script))
        .route(
            "/api/v1/me/progress/archived-free",
            get(super::sync_actions::get_archived_free).put(super::sync_actions::put_archived_free),
        )
}

// One self-hosted static script; its archive mode can never bypass SQL pins.
async fn progress_script() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        {
            static JS: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
                let mut js = String::from(include_str!("../../assets/newboo-free-progress.js"));
                js.push('\n');
                js.push_str(include_str!("../../assets/newboo-archive-toc.js"));
                js
            });
            JS.as_str()
        },
    )
}

#[derive(Deserialize)]
pub(super) struct SavedFreeQuery {
    after: Option<i64>,
}

pub(super) async fn list(
    State(state): State<AuthState>,
    Path(book_id): Path<i64>,
    Query(query): Query<SavedFreeQuery>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let session_digest = tor_accounts::services::security::session_token_digest(&token);
    let result = state
        .bookmarks
        .saved_archive_chapters(actor, book_id, 50, query.after, &session_digest)
        .await;
    match result {
        Ok(page) => private_response(
            pages::saved_archive_list(book_id, &page, state.paid_reader_available),
            &[],
        ),
        Err(LibraryError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

pub(super) async fn read(
    State(state): State<AuthState>,
    Path(chapter_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let session_digest = tor_accounts::services::security::session_token_digest(&token);
    let saved = match state
        .bookmarks
        .pinned_free_chapter(actor, chapter_id, &session_digest)
        .await
    {
        Ok(Some(saved)) if valid_reader_length(&saved.body_markdown) => saved,
        Ok(Some(_)) => return deny(StatusCode::PAYLOAD_TOO_LARGE),
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(LibraryError::Invalid | LibraryError::NotFound) => {
            return deny(StatusCode::NOT_FOUND);
        }
        Err(LibraryError::Unavailable) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let html = if saved.ordinary_deleted {
        // Fetch real actor-scoped pinned and purchased editions around this
        // exact chapter, including gaps and duplicated imported chapter numbers.
        let cursor = SavedArchiveTocCursor {
            chapter_no: saved.chapter_no,
            chapter_id: saved.chapter_id,
        };
        let previous = match state
            .bookmarks
            .saved_archive_toc_window(
                actor,
                saved.book_id,
                25,
                cursor,
                SavedArchiveTocSide::Before,
                &session_digest,
            )
            .await
        {
            Ok(page) => page,
            Err(LibraryError::Invalid | LibraryError::NotFound) => {
                return deny(StatusCode::NOT_FOUND)
            }
            Err(LibraryError::Unavailable) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        };
        let following = match state
            .bookmarks
            .saved_archive_toc_window(
                actor,
                saved.book_id,
                25,
                cursor,
                SavedArchiveTocSide::AtOrAfter,
                &session_digest,
            )
            .await
        {
            Ok(page) => page,
            Err(LibraryError::Invalid | LibraryError::NotFound) => {
                return deny(StatusCode::NOT_FOUND)
            }
            Err(LibraryError::Unavailable) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        };
        // A concurrent pin removal must not turn an old chapter page into a
        // general archive-enumeration session.
        if !following
            .items
            .first()
            .is_some_and(|item| item.chapter_id == saved.chapter_id)
        {
            return deny(StatusCode::NOT_FOUND);
        }
        let mut items = previous.items;
        items.extend(following.items);
        let page = SavedArchivePage {
            items,
            next_cursor: None,
        };
        pages::saved_free_reader_with_paginated_toc(
            &saved,
            &csrf,
            &page,
            state.paid_reader_available,
            previous.next_cursor.as_deref(),
            following.next_cursor.as_deref(),
        )
    } else {
        pages::saved_free_reader(&saved, &csrf)
    };
    let mut response = private_response(html, &cookies);
    // Only the finalized archive opts into the self-hosted cursor script.
    // Earlier saved-free views remain script-free.
    let csp = if saved.ordinary_deleted {
        "default-src 'none'; script-src 'self'; connect-src 'self'; style-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'"
    } else {
        "default-src 'none'; script-src 'none'; style-src 'self'; img-src 'self'; base-uri 'none'; frame-ancestors 'none'"
    };
    response
        .headers_mut()
        .insert("content-security-policy", csp.parse().expect("static CSP"));
    crate::recommendation_http::visits::book_response(response, saved.book_id, 1)
}

#[derive(Deserialize)]
pub(super) struct ArchiveTocQuery {
    before: Option<String>,
    after: Option<String>,
    limit: Option<u16>,
}

#[derive(Serialize)]
struct ArchiveTocPurchased {
    purchase_id: String,
    version_no: i32,
}

#[derive(Serialize)]
struct ArchiveTocItem {
    chapter_id: String,
    chapter_no: i32,
    title: String,
    saved_free_version_no: Option<i32>,
    purchased: Option<ArchiveTocPurchased>,
}

#[derive(Serialize)]
struct ArchiveTocResponse {
    items: Vec<ArchiveTocItem>,
    next_cursor: Option<String>,
}

/// Only readers with a live private saved-free pin for a *finalized* deleted
/// book can page that book's actor-owned archive. Never expose paid text.
pub(super) async fn toc_window(
    State(state): State<AuthState>,
    Path(chapter_id): Path<i64>,
    Query(query): Query<ArchiveTocQuery>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = tor_accounts::services::security::session_token_digest(&token);
    let (cursor, side) = match (query.before.as_deref(), query.after.as_deref()) {
        (Some(raw), None) => (
            SavedArchiveTocCursor::parse(raw),
            SavedArchiveTocSide::Before,
        ),
        (None, Some(raw)) => (
            SavedArchiveTocCursor::parse(raw),
            SavedArchiveTocSide::After,
        ),
        _ => return deny(StatusCode::BAD_REQUEST),
    };
    let Some(cursor) = cursor else {
        return deny(StatusCode::BAD_REQUEST);
    };
    let limit = query.limit.unwrap_or(60);
    if !(1..=60).contains(&limit) {
        return deny(StatusCode::BAD_REQUEST);
    }
    let saved = match state
        .bookmarks
        .pinned_free_chapter(actor, chapter_id, &digest)
        .await
    {
        Ok(Some(saved)) if saved.ordinary_deleted => saved,
        Ok(_) | Err(LibraryError::Invalid | LibraryError::NotFound) => {
            return deny(StatusCode::NOT_FOUND)
        }
        Err(LibraryError::Unavailable) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let page = match state
        .bookmarks
        .saved_archive_toc_window(actor, saved.book_id, limit, cursor, side, &digest)
        .await
    {
        Ok(page) => page,
        Err(LibraryError::Invalid) => return deny(StatusCode::BAD_REQUEST),
        Err(LibraryError::NotFound) => return deny(StatusCode::NOT_FOUND),
        Err(LibraryError::Unavailable) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let items = page
        .items
        .into_iter()
        .map(|chapter| ArchiveTocItem {
            chapter_id: chapter.chapter_id.to_string(),
            chapter_no: chapter.chapter_no,
            title: chapter.chapter_title,
            saved_free_version_no: chapter.saved_free_version_no,
            purchased: chapter.purchased.map(|edition| ArchiveTocPurchased {
                purchase_id: edition.purchase_id.to_string(),
                version_no: edition.version_no,
            }),
        })
        .collect();
    private_response(
        Json(ArchiveTocResponse {
            items,
            next_cursor: page.next_cursor,
        }),
        &[],
    )
}
