//! F-05 / S008: explicit chapter completion in the no-JavaScript reader.
//! A bookmark, chapter GET, or a save at the beginning never proves completion.
use super::{deny, session_user, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_reading::{
    domain::renderer::valid_reader_length,
    services::{
        progress::{is_completed_place, reader_blocks, valid_text_place, ReadingPlace},
        sync::SyncError,
        PublicReadingRepository,
    },
};
use tor_recommendations::repositories::RankingStoreError;

#[derive(Deserialize)]
pub(super) struct CompleteChapterForm {
    csrf_token: String,
    version_id: u64,
    block_index: i32,
    char_offset: i32,
    expected_revision: i64,
}

fn status(error: SyncError) -> StatusCode {
    match error {
        SyncError::Invalid => StatusCode::BAD_REQUEST,
        SyncError::NotFound => StatusCode::NOT_FOUND,
        SyncError::Conflict => StatusCode::CONFLICT,
        SyncError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

pub(super) async fn complete(
    State(state): State<AuthState>,
    Path(id): Path<u64>,
    headers: HeaderMap,
    Form(form): Form<CompleteChapterForm>,
) -> Response {
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, &form.csrf_token)
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(error) => return deny(error),
    };
    // Use the same currently published, readable free edition as /read/{id}.
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
    if valid_text_place(place, &blocks).is_err() || !is_completed_place(place, &blocks) {
        return deny(StatusCode::BAD_REQUEST);
    }
    let (chapter_id, version_id) = match (i64::try_from(id), i64::try_from(form.version_id)) {
        (Ok(chapter_id), Ok(version_id)) => (chapter_id, version_id),
        _ => return deny(StatusCode::NOT_FOUND),
    };
    match state
        .sync
        .save_free(actor, chapter_id, version_id, place, form.expected_revision)
        .await
    {
        Ok(_) => {
            // The C fact and outbox are already committed. Make the ranking
            // immediately visible when possible; the worker retries failures.
            if let Ok(Some(event_id)) = state.sync.completion_event_id(actor, chapter_id).await {
                match state.rankings.accept_completed_chapter(&event_id).await {
                    Ok(_) | Err(RankingStoreError::Conflict) => {}
                    Err(_) => tracing::warn!("ranking delivery queued after completed chapter"),
                }
                if state.achievements.accept_completed_chapter(&event_id).await.is_err() {
                    tracing::warn!("achievement delivery queued after completed chapter");
                }
            }
            private_response(Redirect::to("/me/rankings"), &[])
        }
        Err(error) => deny(status(error)),
    }
}
