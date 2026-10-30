//! A-07: authenticated password rotation without retaining old sessions.
use super::AuthState;
use crate::{auth_cookies::{private_response, SESSION_AGE}, pages};
use axum::{
    extract::{Form, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use std::num::NonZeroU32;
use tor_accounts::services::{
    security::{generate_session_token, hash_password, session_token_digest, verify_password},
    validation::password_change,
};

#[derive(Deserialize)]
pub(super) struct PasswordChangeForm {
    current_password: String,
    new_password: String,
    confirm_password: String,
    csrf_token: Option<String>,
}

pub(super) async fn page(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    match super::session_user(&state, &headers).await {
        Ok(Some(_)) => {}
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return private_response(status, &[]),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(pages::password_change_form(&csrf, None), &cookies)
}

fn error(status: StatusCode, message: &str, csrf: &str) -> Response {
    private_response(
        (status, pages::password_change_form(csrf, Some(message))),
        &[],
    )
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<PasswordChangeForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state.cookies.valid_csrf(
            &headers, form.csrf_token.as_deref().unwrap_or(""),
        )
    {
        return private_response(StatusCode::FORBIDDEN, &[]);
    }
    let Some(user_id) = (match super::session_user(&state, &headers).await {
        Ok(actor) => actor,
        Err(status) => return private_response(status, &[]),
    }) else {
        return private_response(Redirect::to("/login"), &[]);
    };
    let Some(session_token) = state.cookies.session_token(&headers) else {
        return private_response(StatusCode::UNAUTHORIZED, &[]);
    };
    let csrf = form.csrf_token.as_deref().unwrap_or("");
    if let Err(message) = password_change(
        &form.current_password,
        &form.new_password,
        &form.confirm_password,
    ) {
        return error(StatusCode::BAD_REQUEST, message, csrf);
    }
    let attempt_key = session_token_digest(&format!("auth:password-change:{user_id}"));
    match state.accounts.permit_auth_attempt(&attempt_key, 8, 600).await {
        Ok(true) => {}
        Ok(false) => return error(StatusCode::TOO_MANY_REQUESTS, "Слишком много попыток. Повторите позже.", csrf),
        Err(_) => return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    };
    let stored_hash = match state.accounts.password_hash_for_user(user_id).await {
        Ok(Some(hash)) => hash,
        Ok(None) => return private_response(StatusCode::UNAUTHORIZED, &[]),
        Err(_) => return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    };
    let expected_hash = stored_hash.clone();
    let old_password = form.current_password;
    let new_password = form.new_password;
    let new_hash = match state.password_jobs.run(move || {
        if !verify_password(&old_password, &stored_hash) {
            return None;
        }
        hash_password(&new_password).ok()
    }).await {
        Ok(Some(hash)) => hash,
        Ok(None) => return error(StatusCode::UNAUTHORIZED, "Не удалось изменить пароль. Проверьте текущий пароль.", csrf),
        Err(status) => return private_response(status, &[]),
    };
    let token = generate_session_token();
    let updated = state.accounts.rotate_password_and_sessions(
        user_id,
        &session_token_digest(&session_token),
        &expected_hash,
        &new_hash,
        &session_token_digest(&token),
        NonZeroU32::new(SESSION_AGE).expect("session TTL"),
    ).await;
    match updated {
        Ok(true) => {}
        Ok(false) => return error(StatusCode::CONFLICT, "Сеанс или пароль уже изменён. Войдите снова.", csrf),
        Err(_) => return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    }
    // Counter reset failure never restores a revoked password/session.
    let _ = state.accounts.clear_auth_attempt(&attempt_key).await;
    private_response(Redirect::to("/me"), &[state.cookies.session_cookie(&token)])
}
