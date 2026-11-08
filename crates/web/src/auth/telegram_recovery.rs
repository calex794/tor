//! A/S004: authenticated Telegram pairing and public backup-code recovery.
use super::{login_limiter, session_user, AuthState};
use crate::{
    auth_cookies::{private_response, valid_token},
    pages,
};
use axum::{
    extract::{ConnectInfo, Form, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use std::net::SocketAddr;
use tor_accounts::{
    domain::normalize_email,
    services::security::{generate_session_token, session_token_digest, verify_password},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BeginForm {
    current_password: String,
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConfirmForm {
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BackupForm {
    email: String,
    backup_code: String,
    csrf_token: Option<String>,
}

pub(super) async fn link_page(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if !state.telegram_enabled {
        return private_response(StatusCode::NOT_FOUND, &[]);
    }
    let user = match session_user(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return private_response(status, &[]),
    };
    let Some(token) = state.cookies.session_token(&headers) else {
        return private_response(StatusCode::UNAUTHORIZED, &[]);
    };
    let info = match state
        .accounts
        .telegram_link_state(user, &session_token_digest(&token))
        .await
    {
        Ok(info) => info,
        Err(_) => return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(
        pages::telegram_recovery::link_page(
            &csrf,
            info.linked_user_id,
            info.candidate_user_id,
            None,
            None,
        ),
        &cookies,
    )
}

pub(super) async fn begin_link(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<BeginForm>,
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
    if form.current_password.is_empty() || form.current_password.len() > 512 {
        return private_response(StatusCode::BAD_REQUEST, &[]);
    }
    let user = match session_user(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return private_response(status, &[]),
    };
    let Some(token) = state.cookies.session_token(&headers) else {
        return private_response(StatusCode::UNAUTHORIZED, &[]);
    };
    let attempts = session_token_digest(&format!("a07:telegram-link:{user}"));
    match state.accounts.permit_auth_attempt(&attempts, 8, 600).await {
        Ok(true) => {}
        Ok(false) => return private_response(StatusCode::TOO_MANY_REQUESTS, &[]),
        Err(_) => return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    }
    let expected_hash = match state.accounts.password_hash_for_user(user).await {
        Ok(Some(hash)) => hash,
        _ => return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    };
    let input = form.current_password;
    let current = expected_hash.clone();
    let verified = match state
        .password_jobs
        .run(move || verify_password(&input, &current))
        .await
    {
        Ok(ok) => ok,
        Err(status) => return private_response(status, &[]),
    };
    if !verified {
        return private_response(StatusCode::FORBIDDEN, &[]);
    }
    let nonce = generate_session_token();
    let success = state
        .accounts
        .begin_telegram_link(
            user,
            &session_token_digest(&token),
            &expected_hash,
            &session_token_digest(&nonce),
        )
        .await;
    if !matches!(success, Ok(true)) {
        return private_response(StatusCode::CONFLICT, &[]);
    }
    let Some(username) = state.telegram_bot_username.as_deref() else {
        return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]);
    };
    let link = format!("https://t.me/{username}?start=bind_{nonce}");
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(
        pages::telegram_recovery::link_page(&csrf, None, None, Some(&link), None),
        &cookies,
    )
}

pub(super) async fn confirm_link(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<ConfirmForm>,
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
    let user = match session_user(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return private_response(status, &[]),
    };
    let Some(token) = state.cookies.session_token(&headers) else {
        return private_response(StatusCode::UNAUTHORIZED, &[]);
    };
    let codes: [String; 8] = std::array::from_fn(|_| generate_session_token());
    let digests: [[u8; 32]; 8] =
        std::array::from_fn(|i| session_token_digest(&format!("tor:backup:v1:{}", codes[i])));
    let linked = state
        .accounts
        .finish_telegram_link(user, &session_token_digest(&token), &digests)
        .await;
    match linked {
        Ok(Some(_)) => private_response(pages::telegram_recovery::show_backup_codes(&codes), &[]),
        Ok(None) => private_response(StatusCode::CONFLICT, &[]),
        Err(_) => private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    }
}

pub(super) async fn unlink(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<BeginForm>,
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
    if form.current_password.is_empty() || form.current_password.len() > 512 {
        return private_response(StatusCode::BAD_REQUEST, &[]);
    }
    let user = match session_user(&state, &headers).await {
        Ok(Some(id)) => id,
        Ok(None) => return private_response(StatusCode::UNAUTHORIZED, &[]),
        Err(status) => return private_response(status, &[]),
    };
    let Some(token) = state.cookies.session_token(&headers) else {
        return private_response(StatusCode::UNAUTHORIZED, &[]);
    };
    let attempt = session_token_digest(&format!("a07:telegram-unlink:{user}"));
    match state.accounts.permit_auth_attempt(&attempt, 8, 600).await {
        Ok(true) => (),
        Ok(false) => return private_response(StatusCode::TOO_MANY_REQUESTS, &[]),
        Err(_) => return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    }
    let current = match state.accounts.password_hash_for_user(user).await {
        Ok(Some(hash)) => hash,
        _ => return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    };
    let check = current.clone();
    let typed = form.current_password;
    let valid = match state
        .password_jobs
        .run(move || verify_password(&typed, &check))
        .await
    {
        Ok(value) => value,
        Err(status) => return private_response(status, &[]),
    };
    if !valid {
        return private_response(StatusCode::FORBIDDEN, &[]);
    }
    match state
        .accounts
        .unlink_telegram(user, &session_token_digest(&token), &current)
        .await
    {
        Ok(true) => private_response(Redirect::to("/me/security/telegram"), &[]),
        Ok(false) => private_response(StatusCode::CONFLICT, &[]),
        Err(_) => private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    }
}

pub(super) async fn backup_page(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    if !state.telegram_enabled {
        return private_response(StatusCode::NOT_FOUND, &[]);
    }
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(pages::telegram_recovery::backup_form(&csrf, None), &cookies)
}

pub(super) async fn backup_submit(
    State(state): State<AuthState>,
    headers: HeaderMap,
    peer: ConnectInfo<SocketAddr>,
    Form(form): Form<BackupForm>,
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
    let identifier = normalized.as_deref().unwrap_or("invalid-account");
    if let Err(status) =
        login_limiter::limit(&state, "backup-recover", identifier, &headers, peer).await
    {
        return private_response(status, &[]);
    }
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    let generic = || {
        private_response(
            pages::telegram_recovery::backup_form(
                &csrf,
                Some("Если адрес и резервный код верны, воспользуйтесь полученной ссылкой."),
            ),
            &cookies,
        )
    };
    let Some(normalized) = normalized else {
        return generic();
    };
    if !valid_token(&form.backup_code) {
        return generic();
    }
    let proof = session_token_digest(&format!("tor:backup:v1:{}", form.backup_code));
    let token = generate_session_token();
    match state
        .accounts
        .consume_backup_code(&normalized, &proof, &session_token_digest(&token))
        .await
    {
        Ok(true) => private_response(pages::telegram_recovery::backup_ready(&token), &cookies),
        Ok(false) => generic(),
        Err(_) => private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    }
}
