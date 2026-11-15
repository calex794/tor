//! Fresh administrator and CSRF gates for the explicitly versioned model only.
use super::super::{api, pages, Runtime};
use super::{actor, csrf};
use crate::{auth_cookies::deny, pages as html};
use axum::{
    extract::{Form, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    Json,
};
use serde::Deserialize;
use std::time::Duration;
use tor_recommendations::services::contextual::{Action, Control, Status};
mod render;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ControlForm {
    action: Action,
    generation: Option<String>,
    expected_revision: i64,
    csrf_token: String,
}
async fn load(state: &Runtime, headers: &HeaderMap) -> Result<Status, StatusCode> {
    actor(state, headers).await?;
    state
        .store
        .contextual_status()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}
pub(crate) async fn status(State(state): State<Runtime>, headers: HeaderMap) -> Response {
    let response =
        match tokio::time::timeout(Duration::from_millis(950), load(&state, &headers)).await {
            Ok(Ok(status)) => Json(status).into_response(),
            Ok(Err(status)) => deny(status),
            Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
        };
    pages::secure_page(response, &[])
}
pub(crate) async fn page(State(state): State<Runtime>, headers: HeaderMap) -> Response {
    let status =
        match tokio::time::timeout(Duration::from_millis(950), load(&state, &headers)).await {
            Ok(Ok(status)) => Some(status),
            Ok(Err(StatusCode::SERVICE_UNAVAILABLE)) | Err(_) => None,
            Ok(Err(status)) => return pages::secure_page(deny(status), &[]),
        };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let body = render::page(status.as_ref(), &csrf, state.enabled);
    pages::secure_page((if status.is_some(){StatusCode::OK}else{StatusCode::SERVICE_UNAVAILABLE},html::layout("\u{41a}\u{43e}\u{43d}\u{442}\u{435}\u{43a}\u{441}\u{442}\u{43d}\u{44b}\u{435} \u{440}\u{435}\u{43a}\u{43e}\u{43c}\u{435}\u{43d}\u{434}\u{430}\u{446}\u{438}\u{438}","metadata_manual_v1",&body)),&cookies)
}
async fn execute(
    state: Runtime,
    headers: HeaderMap,
    body: Control,
    token: String,
    is_form: bool,
) -> Response {
    let operation = async {
        if let Err(status) = csrf(&state, &headers, &token) {
            return deny(status);
        }
        if let Err(status) = actor(&state, &headers).await {
            return deny(status);
        }
        match state.store.control_contextual(&body).await {
            Ok(_) if is_form => Redirect::to("/me/recommendations/contextual").into_response(),
            Ok(receipt) => Json(receipt).into_response(),
            Err(error) => api::error(error),
        }
    };
    let response = tokio::time::timeout(Duration::from_millis(950), operation)
        .await
        .unwrap_or_else(|_| deny(StatusCode::SERVICE_UNAVAILABLE));
    pages::secure_page(response, &[])
}
pub(crate) async fn control(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Json(body): Json<Control>,
) -> Response {
    let token = headers
        .get("x-csrf-token")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_owned();
    execute(state, headers, body, token, false).await
}
pub(crate) async fn form(
    State(state): State<Runtime>,
    headers: HeaderMap,
    Form(body): Form<ControlForm>,
) -> Response {
    let generation = body.generation.filter(|s| !s.is_empty());
    execute(
        state,
        headers,
        Control {
            action: body.action,
            generation,
            expected_revision: body.expected_revision,
        },
        body.csrf_token,
        true,
    )
    .await
}
