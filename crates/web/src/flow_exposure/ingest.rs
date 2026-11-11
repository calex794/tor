use super::Runtime;
use crate::auth_cookies::private_response;
use axum::{
    extract::{rejection::JsonRejection, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    Json,
};
use std::{sync::atomic::Ordering, time::Duration};
use tor_accounts::services::security::session_token_digest;
use tor_recommendations::domain::exposure::{ExposureError, ObservationEvent};

pub(super) async fn event(
    State(state): State<Runtime>,
    headers: HeaderMap,
    body: Result<Json<ObservationEvent>, JsonRejection>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || headers
            .get("x-tor-observation")
            .and_then(|h| h.to_str().ok())
            != Some("1")
    {
        return private_response(StatusCode::FORBIDDEN, &[]);
    }
    let event = match body {
        Ok(Json(event)) => event,
        Err(error) => return private_response(error.status(), &[]),
    };
    let work = async {
        event.validate()?;
        let epoch = state.gate.epoch().ok_or(ExposureError::Unavailable)?;
        let context = state.store.describe(&event.proof).await?;
        if context.card.editorial.is_some() {
            let Some(guard) = state.guard.as_ref() else {
                return Err(ExposureError::Forbidden);
            };
            if !guard
                .allowed(&context.card)
                .await
                .map_err(|_| ExposureError::Unavailable)?
            {
                return Err(ExposureError::Forbidden);
            }
        }
        let actor = match state.cookies.session_token(&headers) {
            Some(token) => Some(
                state
                    .accounts
                    .active_session_user(&session_token_digest(&token))
                    .await
                    .map_err(|_| ExposureError::Unavailable)?
                    .ok_or(ExposureError::Forbidden)?,
            ),
            None => None,
        };
        state
            .store
            .record_in_epoch(&event, &context, actor, epoch)
            .await
    };
    let status = match tokio::time::timeout(Duration::from_millis(950), work).await {
        Ok(Ok(_)) => StatusCode::NO_CONTENT,
        Ok(Err(ExposureError::Invalid)) => StatusCode::BAD_REQUEST,
        // Missing may be an issuance queue race. The client retries only a fixed number of times.
        Ok(Err(ExposureError::Missing | ExposureError::TooEarly)) => StatusCode::TOO_EARLY,
        Ok(Err(ExposureError::Expired)) => StatusCode::GONE,
        Ok(Err(ExposureError::Forbidden)) => StatusCode::FORBIDDEN,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    if status == StatusCode::SERVICE_UNAVAILABLE {
        state.gate.disable();
        state.counters.failed.fetch_add(1, Ordering::Release);
    }
    private_response(status, &[])
}
