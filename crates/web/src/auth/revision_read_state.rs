//! C-18: authenticated per-event V2 read/unread, not a chapter opening.
use super::{deny, session_user, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_reading::repositories::StagedRevisionError;

#[derive(Deserialize)]
pub(super) struct RevisionReadForm {
    csrf_token: Option<String>,
    state: String,
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Form(form): Form<RevisionReadForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !form
            .csrf_token
            .as_deref()
            .is_some_and(|csrf| state.cookies.valid_csrf(&headers, csrf))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let read = match form.state.as_str() {
        "read" => true,
        "unread" => false,
        _ => return deny(StatusCode::UNPROCESSABLE_ENTITY),
    };
    match state
        .staged_revisions
        .set_read_state(actor, &id, read)
        .await
    {
        Ok(true) => private_response(Redirect::to("/me/library#revised-chapters"), &[]),
        Ok(false) => deny(StatusCode::NOT_FOUND),
        Err(StagedRevisionError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(StagedRevisionError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}
