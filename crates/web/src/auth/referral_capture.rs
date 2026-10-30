//! S052: public referral landing, without privileged book/account access.
use super::AuthState;
use crate::auth_cookies::private_response;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use tor_accounts::services::security::{generate_session_token, session_token_digest};
use tor_referrals::domain::valid_code;
use tor_referrals::repositories::postgres::CaptureResult;

pub(super) async fn capture(
    State(state): State<AuthState>,
    Path(code): Path<String>,
    headers: HeaderMap,
) -> Response {
    if !valid_code(&code) {
        return private_response(Redirect::to("/"), &[]);
    }
    // Never record a new referral for an authenticated user.
    if let Some(session) = state.cookies.session_token(&headers) {
        let digest = session_token_digest(&session);
        match state.accounts.active_session_user(&digest).await {
            Ok(Some(_)) => return private_response(Redirect::to("/"), &[]),
            Err(_) => return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
            Ok(None) => {}
        }
    }
    let existing = state.cookies.guest_token(&headers);
    let guest_token = existing.clone().unwrap_or_else(generate_session_token);
    let hash = session_token_digest(&guest_token);
    let path = format!("/r/{code}");
    let result = state
        .referrals
        .capture(&hash, &code, "direct_ref_link", &path)
        .await;
    match result {
        Ok(CaptureResult::Saved | CaptureResult::Existing) => {
            let cookies = if existing.is_none() {
                vec![state.cookies.guest_cookie(&guest_token)]
            } else {
                Vec::new()
            };
            private_response(Redirect::to("/register"), &cookies)
        }
        Ok(CaptureResult::Invalid) => private_response(Redirect::to("/"), &[]),
        Err(_) => private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    }
}

async fn alias_redirect(state: &AuthState, prefix: &str, alias: &str) -> Response {
    if !tor_referrals::domain::valid_alias(alias) {
        return private_response(Redirect::to("/"), &[]);
    }
    match state.referrals.resolve_active_alias(prefix, alias).await {
        Ok(Some(code)) => private_response(Redirect::to(&format!("/r/{code}")), &[]),
        Ok(None) => private_response(Redirect::to("/"), &[]),
        Err(_) => private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    }
}

pub(super) async fn go(
    State(state): State<AuthState>,
    Path(alias): Path<String>,
) -> Response {
    alias_redirect(&state, "go", &alias).await
}

pub(super) async fn promo(
    State(state): State<AuthState>,
    Path(alias): Path<String>,
) -> Response {
    alias_redirect(&state, "promo", &alias).await
}
