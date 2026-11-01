//! E-05/S014: reader-private creator list and unfollow by canonical actor.
#![forbid(unsafe_code)]

use super::{deny, session_user, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
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
