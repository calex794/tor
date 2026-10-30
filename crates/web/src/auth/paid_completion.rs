//! H/C/F-05: an explicit browser completion for a D-owned encrypted V1.
//! Unlike a S3 grant/GET or saved position this requires the sealed G anchor.
use super::{
    deny, session_user,
    sync_actions::{id, status},
    AuthState,
};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    Json,
};
use serde::{Deserialize, Serialize};
use tor_reading::repositories::sync::PurchasedFinish;
use tor_reading::services::progress::ReadingPlace;

// The actual cookie and Origin predicates are local so the RIL reviewer
// checks the same security boundary that each handler calls.
fn csrf(state: &AuthState, headers: &HeaderMap) -> bool {
    state.cookies.same_origin(headers)
        && headers
            .get("x-csrf-token")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| state.cookies.valid_csrf(headers, value))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Lookup {
    purchase_id: String,
    chapter_id: String,
    version_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Finish {
    purchase_id: String,
    chapter_id: String,
    version_id: String,
    block_id: i32,
    char_offset: i32,
    expected_revision: i64,
    plaintext_sha256: String,
}

#[derive(Serialize)]
struct CompletionState {
    available: bool,
    completed: bool,
}

#[derive(Serialize)]
struct CompletionResult {
    status: &'static str,
}

pub(super) async fn get(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Query(query): Query<Lookup>,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(error) => return deny(error),
    };
    let (Ok(purchase_id), Ok(chapter_id), Ok(version_id)) = (
        id(&query.purchase_id),
        id(&query.chapter_id),
        id(&query.version_id),
    ) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    match state
        .sync
        .purchased_s3_completion_state(actor, purchase_id, chapter_id, version_id)
        .await
    {
        Ok((available, completed)) => private_response(
            Json(CompletionState {
                available,
                completed,
            }),
            &[],
        ),
        Err(error) => deny(status(error)),
    }
}

pub(super) async fn post(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Json(command): Json<Finish>,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(error) => return deny(error),
    };
    if !csrf(&state, &headers) {
        return deny(StatusCode::FORBIDDEN);
    }
    let (Ok(purchase_id), Ok(chapter_id), Ok(version_id)) = (
        id(&command.purchase_id),
        id(&command.chapter_id),
        id(&command.version_id),
    ) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    let place = ReadingPlace {
        block_index: command.block_id,
        char_offset: command.char_offset,
    };
    match state
        .sync
        .complete_purchased_s3(
            actor,
            PurchasedFinish {
                purchase_id,
                chapter_id,
                version_id,
                place,
                expected_revision: command.expected_revision,
                plaintext_sha256: &command.plaintext_sha256,
            },
        )
        .await
    {
        Ok((event_id, newly_completed)) => {
            // A successful C commit is sufficient for the reader response.
            // F uses the existing durable outbox if immediate delivery fails.
            if state
                .rankings
                .accept_completed_chapter(&event_id)
                .await
                .is_err()
            {
                tracing::warn!("paid encrypted chapter ranking delivery remains queued");
            }
            if state.achievements.accept_completed_chapter(&event_id).await.is_err() {
                tracing::warn!("paid chapter achievement delivery remains queued");
            }
            private_response(
                Json(CompletionResult {
                    status: if newly_completed {
                        "completed"
                    } else {
                        "already_completed"
                    },
                }),
                &[],
            )
        }
        Err(error) => deny(status(error)),
    }
}
