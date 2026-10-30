//! NEwBOO profile/mail friendship POST actions, session-scoped and CSRF guarded.
use super::{deny, session_user, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Form, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_social::repositories::{friendships::PgFriendships, mail::PgMail};
use tor_social::services::MailService;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ActionForm {
    csrf_token: Option<String>,
}

fn csrf(state: &AuthState, headers: &HeaderMap, value: Option<&str>) -> Result<(), StatusCode> {
    if !state.cookies.same_origin(headers)
        || !value.is_some_and(|v| state.cookies.valid_csrf(headers, v))
    {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(())
}

fn redirect_to_source(state: &AuthState, headers: &HeaderMap) -> Response {
    // Only relative paths under existing profile/mail pages are accepted.
    // A forged Referer must never turn a successful POST into an open redirect.
    let destination = headers.get(header::REFERER)
        .and_then(|h| h.to_str().ok())
        .and_then(|value| value.strip_prefix(&state.cookies.origin))
        .filter(|path| path.starts_with("/users/") || path.starts_with("/mail/"))
        .unwrap_or("/mail");
    private_response(Redirect::to(destination), &[])
}

async fn actor(state: &AuthState, headers: &HeaderMap) -> Result<i64, StatusCode> {
    session_user(state, headers).await?.ok_or(StatusCode::UNAUTHORIZED)
}

pub(super) async fn request(
    State(state): State<AuthState>,
    Path(peer): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<ActionForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(actor) => actor,
        Err(status) => return deny(status),
    };
    if let Err(status) = csrf(&state, &headers, form.csrf_token.as_deref()) { return deny(status); }
    match PgFriendships::new(state.accounts.pool().clone()).request(actor, peer).await {
        Ok(_) => redirect_to_source(&state, &headers),
        Err(e) => deny(super::mail::status(e)),
    }
}

pub(super) async fn remove(
    State(state): State<AuthState>,
    Path(peer): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<ActionForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(actor) => actor,
        Err(status) => return deny(status),
    };
    if let Err(status) = csrf(&state, &headers, form.csrf_token.as_deref()) { return deny(status); }
    match PgFriendships::new(state.accounts.pool().clone()).remove(actor, peer).await {
        Ok(()) => redirect_to_source(&state, &headers),
        Err(e) => deny(super::mail::status(e)),
    }
}

pub(super) async fn block(
    State(state): State<AuthState>,
    Path(peer): Path<i64>,
    headers: HeaderMap,
    Form(form): Form<ActionForm>,
) -> Response {
    let actor = match actor(&state, &headers).await {
        Ok(actor) => actor,
        Err(status) => return deny(status),
    };
    if let Err(status) = csrf(&state, &headers, form.csrf_token.as_deref()) { return deny(status); }
    match MailService::new(PgMail::new(state.accounts.pool().clone()))
        .set_block(actor, peer, true).await
    {
        Ok(()) => redirect_to_source(&state, &headers),
        Err(e) => deny(super::mail::status(e)),
    }
}
