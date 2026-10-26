//! Revision-checked cross-device position API. No paid text is serialized.
#![forbid(unsafe_code)]

use super::{deny, session_user, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    Json,
};
use serde::{Deserialize, Serialize};
use tor_reading::services::{
    progress::{reader_blocks, valid_text_place, ReadingPlace},
    sync::{positive_id, StoredProgress, SyncError, SyncReceipt},
    PublicReadingRepository,
};

#[derive(Deserialize)]
pub(super) struct FreeLookup {
    chapter_id: String,
}

#[derive(Deserialize)]
pub(super) struct PurchasedLookup {
    purchase_id: String,
    chapter_id: String,
    version_id: String,
}

#[derive(Deserialize)]
pub(super) struct FreeSave {
    chapter_id: String,
    version_id: String,
    block_id: i32,
    char_offset: i32,
    expected_revision: i64,
}

#[derive(Deserialize)]
pub(super) struct PurchasedSave {
    purchase_id: String,
    chapter_id: String,
    version_id: String,
    block_id: i32,
    char_offset: i32,
    expected_revision: i64,
}

#[derive(Serialize)]
struct PositionResponse {
    chapter_id: String,
    version_id: String,
    block_id: Option<i32>,
    char_offset: Option<i32>,
    revision: i64,
    updated_at: Option<String>,
}

fn status(error: SyncError) -> StatusCode {
    match error {
        SyncError::Invalid => StatusCode::UNPROCESSABLE_ENTITY,
        SyncError::NotFound => StatusCode::NOT_FOUND,
        SyncError::Conflict => StatusCode::CONFLICT,
        SyncError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

fn id(raw: &str) -> Result<i64, StatusCode> {
    positive_id(raw).map_err(status)
}

fn csrf(state: &AuthState, headers: &HeaderMap) -> bool {
    state.cookies.same_origin(headers)
        && headers
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| state.cookies.valid_csrf(headers, value))
}

fn response(
    chapter_id: i64,
    version_id: i64,
    place: Option<ReadingPlace>,
    receipt: Option<SyncReceipt>,
) -> PositionResponse {
    PositionResponse {
        chapter_id: chapter_id.to_string(),
        version_id: version_id.to_string(),
        block_id: place.map(|p| p.block_index),
        char_offset: place.map(|p| p.char_offset),
        revision: receipt.as_ref().map_or(0, |r| r.revision),
        updated_at: receipt.map(|r| r.updated_at),
    }
}

fn actual_state(
    chapter_id: i64,
    version_id: i64,
    state: Option<StoredProgress>,
) -> PositionResponse {
    match state {
        Some(saved) => response(
            chapter_id,
            version_id,
            (saved.chapter_id == chapter_id && saved.version_id == version_id)
                .then_some(saved.place),
            Some(saved.receipt),
        ),
        None => response(chapter_id, version_id, None, None),
    }
}

pub(super) async fn get_free(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<FreeLookup>,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(error) => return deny(error),
    };
    let chapter_id = match id(&query.chapter_id) {
        Ok(id) => id,
        Err(error) => return deny(error),
    };
    let chapter = match state.reader.public_chapter(chapter_id as u64).await {
        Ok(Some(chapter)) => chapter,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let version_id = match i64::try_from(chapter.version_id) {
        Ok(id) => id,
        Err(_) => return deny(StatusCode::NOT_FOUND),
    };
    match state.sync.free_state(actor, chapter_id).await {
        Ok(saved) => private_response(Json(actual_state(chapter_id, version_id, saved)), &[]),
        Err(error) => deny(status(error)),
    }
}

pub(super) async fn put_free(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(command): Json<FreeSave>,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(error) => return deny(error),
    };
    if !csrf(&state, &headers) {
        return deny(StatusCode::FORBIDDEN);
    }
    let (chapter_id, version_id) = match (id(&command.chapter_id), id(&command.version_id)) {
        (Ok(chapter_id), Ok(version_id)) => (chapter_id, version_id),
        (Err(error), _) | (_, Err(error)) => return deny(error),
    };
    let chapter = match state.reader.public_chapter(chapter_id as u64).await {
        Ok(Some(chapter)) => chapter,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if chapter.version_id != version_id as u64 {
        return deny(StatusCode::CONFLICT);
    }
    let place = ReadingPlace {
        block_index: command.block_id,
        char_offset: command.char_offset,
    };
    if valid_text_place(place, &reader_blocks(&chapter.body_markdown)).is_err() {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    }
    match state
        .sync
        .save_free(
            actor,
            chapter_id,
            version_id,
            place,
            command.expected_revision,
        )
        .await
    {
        Ok(receipt) => private_response(
            Json(response(chapter_id, version_id, Some(place), Some(receipt))),
            &[],
        ),
        Err(error) => deny(status(error)),
    }
}

pub(super) async fn get_purchased(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<PurchasedLookup>,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(error) => return deny(error),
    };
    let (purchase_id, chapter_id, version_id) = match (
        id(&query.purchase_id),
        id(&query.chapter_id),
        id(&query.version_id),
    ) {
        (Ok(purchase_id), Ok(chapter_id), Ok(version_id)) => (purchase_id, chapter_id, version_id),
        (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => return deny(error),
    };
    match state
        .sync
        .purchased_state(actor, purchase_id, chapter_id, version_id)
        .await
    {
        Ok(saved) => private_response(Json(actual_state(chapter_id, version_id, saved)), &[]),
        Err(error) => deny(status(error)),
    }
}

pub(super) async fn put_purchased(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(command): Json<PurchasedSave>,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(error) => return deny(error),
    };
    if !csrf(&state, &headers) {
        return deny(StatusCode::FORBIDDEN);
    }
    let (purchase_id, chapter_id, version_id) = match (
        id(&command.purchase_id),
        id(&command.chapter_id),
        id(&command.version_id),
    ) {
        (Ok(purchase_id), Ok(chapter_id), Ok(version_id)) => (purchase_id, chapter_id, version_id),
        (Err(error), _, _) | (_, Err(error), _) | (_, _, Err(error)) => return deny(error),
    };
    let body = match state
        .sync
        .purchased_body_for_validation(actor, purchase_id, chapter_id, version_id)
        .await
    {
        Ok(Some(body)) => body,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(error) => return deny(status(error)),
    };
    let place = ReadingPlace {
        block_index: command.block_id,
        char_offset: command.char_offset,
    };
    if valid_text_place(place, &reader_blocks(&body)).is_err() {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    }
    match state
        .sync
        .save_purchased(
            actor,
            purchase_id,
            chapter_id,
            version_id,
            place,
            command.expected_revision,
        )
        .await
    {
        Ok(receipt) => private_response(
            Json(response(chapter_id, version_id, Some(place), Some(receipt))),
            &[],
        ),
        Err(error) => deny(status(error)),
    }
}
