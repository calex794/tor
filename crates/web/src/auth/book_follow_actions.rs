//! Authenticated, CSRF-protected book-follow actions.
#![forbid(unsafe_code)]
use super::{deny, session_user, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Form, Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_reading::repositories::FollowError;

#[derive(Deserialize)]
pub(super) struct FollowForm {
    csrf_token: Option<String>,
}

fn follow_status(error: FollowError) -> StatusCode {
    match error {
        FollowError::Invalid => StatusCode::BAD_REQUEST,
        FollowError::NotFound => StatusCode::NOT_FOUND,
        FollowError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

async fn write(
    state: &AuthState,
    headers: &HeaderMap,
    id: u64,
    form: &FollowForm,
    add: bool,
) -> Result<(), StatusCode> {
    if !state.cookies.same_origin(headers)
        || !state.cookies.valid_csrf(headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return Err(StatusCode::FORBIDDEN);
    }
    let actor = session_user(state, headers).await?.ok_or(StatusCode::UNAUTHORIZED)?;
    let book_id = i64::try_from(id).map_err(|_| StatusCode::BAD_REQUEST)?;
    let result = if add {
        state.book_follows.follow(actor, book_id).await
    } else {
        state.book_follows.unfollow(actor, book_id).await
    };
    result.map_err(follow_status)
}

pub(super) async fn follow(
    State(state): State<AuthState>,
    Path(id): Path<u64>,
    headers: HeaderMap,
    Form(form): Form<FollowForm>,
) -> Response {
    match write(&state, &headers, id, &form, true).await {
        Ok(()) => private_response(Redirect::to(&format!("/books/{id}")), &[]),
        Err(status) => deny(status),
    }
}

pub(super) async fn unfollow(
    State(state): State<AuthState>,
    Path(id): Path<u64>,
    headers: HeaderMap,
    Form(form): Form<FollowForm>,
) -> Response {
    match write(&state, &headers, id, &form, false).await {
        Ok(()) => private_response(Redirect::to(&format!("/books/{id}")), &[]),
        Err(status) => deny(status),
    }
}
