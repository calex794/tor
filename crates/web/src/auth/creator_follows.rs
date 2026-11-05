//! E-05/S014: reader-private creator list and unfollow by canonical actor.
#![forbid(unsafe_code)]

use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use tor_social::services::follow::CreatorFollowError;

#[derive(Default, Deserialize)]
pub(super) struct Cursor {
    after: Option<i64>,
}

#[derive(Deserialize)]
pub(super) struct CsrfForm {
    csrf_token: Option<String>,
}

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/me/creators", get(page))
        .route("/me/creators/{id}/unfollow", post(unfollow))
        .route("/users/{target_user_id}/subscribe", post(subscribe_profile))
        .route(
            "/users/{target_user_id}/unsubscribe",
            post(unsubscribe_profile),
        )
}

fn profile_path(name: &str) -> String {
    let mut path = String::from("/users/");
    for byte in name.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            path.push(char::from(byte));
        } else {
            path.push_str(&format!("%{byte:02X}"));
        }
    }
    path
}

async fn write(
    state: &AuthState,
    target_id: u64,
    headers: &HeaderMap,
    form: &CsrfForm,
    subscribe: bool,
) -> Response {
    if !state.cookies.same_origin(headers)
        || !state
            .cookies
            .valid_csrf(headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(state, headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let creator = match i64::try_from(target_id) {
        Ok(creator) if creator > 0 => creator,
        _ => return deny(StatusCode::BAD_REQUEST),
    };
    let result = if subscribe {
        state
            .creator_follows
            .follow_profile(actor, creator)
            .await
            .map(|_| ())
    } else {
        state.creator_follows.unfollow(actor, creator).await
    };
    if let Err(error) = result {
        return deny(error_status(error));
    }
    let destination = match state.accounts.user_display_name(creator).await {
        Ok(Some(name)) => profile_path(&name),
        Ok(None) => "/me/creators".to_owned(),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    private_response(Redirect::to(&destination), &[])
}

pub(super) async fn subscribe_profile(
    State(state): State<AuthState>,
    Path(target_id): Path<u64>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> Response {
    write(&state, target_id, &headers, &form, true).await
}

pub(super) async fn unsubscribe_profile(
    State(state): State<AuthState>,
    Path(target_id): Path<u64>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> Response {
    write(&state, target_id, &headers, &form, false).await
}

fn error_status(error: CreatorFollowError) -> StatusCode {
    match error {
        CreatorFollowError::Invalid => StatusCode::BAD_REQUEST,
        CreatorFollowError::NotFound => StatusCode::NOT_FOUND,
        CreatorFollowError::Forbidden => StatusCode::FORBIDDEN,
        CreatorFollowError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
    }
}

pub(super) async fn page(
    State(state): State<AuthState>,
    Query(query): Query<Cursor>,
    headers: HeaderMap,
) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let mut rows = match state.creator_follows.list_active(actor, query.after).await {
        Ok(rows) => rows,
        Err(error) => return deny(error_status(error)),
    };
    let next = if rows.len() > 20 {
        rows.truncate(20);
        rows.last().map(|row| row.period_id)
    } else {
        None
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(pages::creator_follows(&rows, next, &csrf), &cookies)
}

pub(super) async fn unfollow(
    State(state): State<AuthState>,
    Path(id): Path<u64>,
    headers: HeaderMap,
    Form(form): Form<CsrfForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let creator = match i64::try_from(id) {
        Ok(id) if id > 0 => id,
        _ => return deny(StatusCode::BAD_REQUEST),
    };
    match state.creator_follows.unfollow(actor, creator).await {
        Ok(()) => private_response(Redirect::to("/me/creators"), &[]),
        Err(error) => deny(error_status(error)),
    }
}
