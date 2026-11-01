//! Organic discovery click receipt: authenticated POST, never a GET side effect.
use super::{api, Runtime};
use crate::auth_cookies::{deny, private_response};
use axum::{
    extract::{Form, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde::Deserialize;
use tor_accounts::services::security::session_token_digest;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Activation {
    ticket: String,
    csrf_token: String,
}

async fn actor(state: &Runtime, headers: &HeaderMap) -> Result<(i64, [u8; 32]), StatusCode> {
    let token = state
        .cookies
        .session_token(headers)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let digest = session_token_digest(&token);
    let id = state
        .accounts
        .active_session_user(&digest)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    Ok((id, digest))
}

fn csrf(state: &Runtime, headers: &HeaderMap, posted: &str) -> Result<(), StatusCode> {
    if !state.cookies.same_origin(headers) || !state.cookies.valid_csrf(headers, posted) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

pub(super) async fn activate(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Form(body): Form<Activation>,
) -> Response {
    if !state.enabled || !state.collection || !state.organic {
        return deny(StatusCode::FORBIDDEN);
    }
    if let Err(status) = csrf(&state, &headers, &body.csrf_token) {
        return deny(status);
    }
    let (actor, session) = match actor(&state, &headers).await {
        Ok(identity) => identity,
        Err(status) => return deny(status),
    };
    match state.store.activate_organic(actor, &session, &body.ticket).await {
        Ok(()) => private_response(StatusCode::NO_CONTENT, &[]),
        Err(error) => api::error(error),
    }
}

/// Without JS, links remain regular links and a GET never fabricates a click.
pub(super) async fn script() -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=300"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        include_str!("../../assets/organic_discovery.js"),
    )
        .into_response()
}
