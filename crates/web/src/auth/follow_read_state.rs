//! C-16: private acknowledgement of a release in the followed-books feed.
//! Only a signed-in follower may update their own notification marker.
use super::{deny, session_user, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_reading::repositories::FollowUpdatesError;

#[derive(Deserialize)]
pub(super) struct ReleaseReadStateForm {
    csrf_token: Option<String>,
    state: String,
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<ReleaseReadStateForm>,
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
    match state.follow_updates.set_read_state(actor, id, read).await {
        Ok(true) => private_response(Redirect::to("/me/library#new-chapters"), &[]),
        Ok(false) => deny(StatusCode::NOT_FOUND),
        Err(FollowUpdatesError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(FollowUpdatesError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}
