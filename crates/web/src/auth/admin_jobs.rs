//! S066 G-07: administrator dead-letter retry over the existing G outbox.
//! No worker is started by this endpoint; no recipient delivery is performed here.
use super::AuthState;
use crate::auth_cookies::{deny, private_response};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::post,
    Json, Router,
};
use serde::Serialize;
use tor_accounts::services::security::session_token_digest;
use tor_ingest::repositories::outbox_retry::{AdminRetryError, PgOutboxAdminRetry};

#[derive(Serialize)]
struct RetryAccepted {
    event_id: String,
    status: &'static str,
    previous_attempts: i32,
    lease_epoch: i64,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new().route("/api/v1/admin/jobs/{id}/retry", post(retry))
}

async fn retry(
    State(state): State<AuthState>,
    Path(event_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(token) = state.cookies.session_token(&headers) else {
        return deny(StatusCode::UNAUTHORIZED);
    };
    let digest = session_token_digest(&token);
    let actor = match state.accounts.session_actor(&digest).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if !actor.administrator {
        return deny(StatusCode::FORBIDDEN);
    }

    // Browser double-submit CSRF plus Origin/Fetch-Site protection; a plain
    // same-site cookie alone never authorizes a state-changing operator API.
    let csrf = headers
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !state.cookies.same_origin(&headers) || !state.cookies.valid_csrf(&headers, csrf) {
        return deny(StatusCode::FORBIDDEN);
    }

    match PgOutboxAdminRetry::new(state.accounts.pool().clone())
        .retry_dead_letter(&digest, &event_id)
        .await
    {
        Ok(receipt) => private_response(
            (
                StatusCode::ACCEPTED,
                Json(RetryAccepted {
                    event_id: receipt.event_id,
                    status: "queued",
                    previous_attempts: receipt.previous_attempts,
                    lease_epoch: receipt.lease_epoch,
                }),
            ),
            &[],
        ),
        Err(AdminRetryError::InvalidId) => deny(StatusCode::BAD_REQUEST),
        Err(AdminRetryError::Forbidden) => deny(StatusCode::FORBIDDEN),
        Err(AdminRetryError::NotFound) => deny(StatusCode::NOT_FOUND),
        Err(AdminRetryError::NotRetryable) => deny(StatusCode::CONFLICT),
        Err(AdminRetryError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}
