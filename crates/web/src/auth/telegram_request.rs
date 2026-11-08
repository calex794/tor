//! A/S004: browser-initiated Telegram challenge and browser-only reset exchange.
use super::{login_limiter, AuthState};
use crate::{
    auth_cookies::private_response,
    pages,
};
use axum::{
    extract::{ConnectInfo, Form, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;
use std::net::SocketAddr;
use tor_accounts::{
    domain::normalize_email,
    services::security::{generate_session_token, session_token_digest},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RequestForm {
    email: String,
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CompleteForm {
    csrf_token: Option<String>,
}

pub(super) async fn page(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if !state.telegram_enabled {
        return private_response(StatusCode::NOT_FOUND, &[]);
    }
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let view = if let Some(secret) = state.cookies.telegram_recovery_token(&headers) {
        match state
            .accounts
            .browser_telegram_recovery(&session_token_digest(&secret))
            .await
        {
            Ok(view) => view,
            Err(_) => return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
        }
    } else {
        None
    };
    let content = match view {
        Some(request) => pages::telegram_request::challenge(
            &csrf,
            &request.match_code,
            &request.status,
        ),
        None => pages::telegram_request::begin_form(&csrf),
    };
    private_response(content, &cookies)
}

pub(super) async fn request(
    State(state): State<AuthState>,
    headers: HeaderMap,
    peer: ConnectInfo<SocketAddr>,
    Form(form): Form<RequestForm>,
) -> Response {
    if !state.telegram_enabled {
        return private_response(StatusCode::NOT_FOUND, &[]);
    }
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return private_response(StatusCode::FORBIDDEN, &[]);
    }
    let normalized = normalize_email(&form.email).ok();
    if let Err(status) = login_limiter::limit(
        &state,
        "telegram-request",
        normalized.as_deref().unwrap_or("invalid-email"),
        &headers,
        peer,
    )
    .await
    {
        return private_response(status, &[]);
    }
    let browser_secret = generate_session_token();
    let comparison_seed = generate_session_token();
    let match_code = &comparison_seed[..6];
    if state
        .accounts
        .begin_browser_telegram_recovery(
            normalized.as_deref(),
            &session_token_digest(&browser_secret),
            match_code,
        )
        .await
        .is_err()
    {
        return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]);
    }
    let (csrf, mut cookies) = state.cookies.csrf_for_page(&headers);
    cookies.push(state.cookies.telegram_recovery_cookie(&browser_secret));
    private_response(
        pages::telegram_request::challenge(&csrf, match_code, "pending"),
        &cookies,
    )
}

pub(super) async fn complete(
    State(state): State<AuthState>,
    headers: HeaderMap,
    peer: ConnectInfo<SocketAddr>,
    Form(form): Form<CompleteForm>,
) -> Response {
    if !state.telegram_enabled {
        return private_response(StatusCode::NOT_FOUND, &[]);
    }
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return private_response(StatusCode::FORBIDDEN, &[]);
    }
    let Some(browser_secret) = state.cookies.telegram_recovery_token(&headers) else {
        return private_response(StatusCode::CONFLICT, &[]);
    };
    if let Err(status) = login_limiter::limit(
        &state,
        "telegram-complete",
        &browser_secret,
        &headers,
        peer,
    )
    .await
    {
        return private_response(status, &[]);
    }
    let reset_secret = generate_session_token();
    match state
        .accounts
        .exchange_browser_telegram_approval(
            &session_token_digest(&browser_secret),
            &session_token_digest(&reset_secret),
        )
        .await
    {
        Ok(true) => private_response(
            pages::telegram_recovery::backup_ready(&reset_secret),
            &[state.cookies.clear_telegram_recovery_cookie()],
        ),
        Ok(false) => private_response(StatusCode::CONFLICT, &[]),
        Err(_) => private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    }
}
