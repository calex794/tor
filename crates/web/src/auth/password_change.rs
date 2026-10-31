//! A-07: HTML and JSON share one credential rotation command.
use super::{access_audit, AuthState};
use crate::{
    auth_cookies::{private_response, SESSION_AGE},
    pages,
};
use axum::{
    extract::{ConnectInfo, Form, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    Json,
};
use serde::Deserialize;
use std::{net::SocketAddr, num::NonZeroU32};
use tor_accounts::{
    domain::access_audit::{LoginAuditContext, LoginEventKind},
    repositories::credentials::PasswordRotation,
    services::{
        security::{generate_session_token, hash_password, session_token_digest, verify_password},
        validation::password_change,
    },
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PasswordChangeForm {
    current_password: String,
    new_password: String,
    confirm_password: String,
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ApiPasswordForm {
    current_password: String,
    new_password: String,
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

async fn rotate(
    state: &AuthState,
    headers: &HeaderMap,
    user_id: i64,
    form: PasswordChangeForm,
    audit: LoginAuditContext,
) -> Result<String, (StatusCode, &'static str)> {
    password_change(
        &form.current_password,
        &form.new_password,
        &form.confirm_password,
    )
    .map_err(|message| (StatusCode::BAD_REQUEST, message))?;
    let session = state
        .cookies
        .session_token(headers)
        .ok_or((StatusCode::UNAUTHORIZED, "Войдите снова"))?;
    let key = session_token_digest(&format!("auth:password-change:{user_id}"));
    let permitted = state
        .accounts
        .permit_auth_attempt(&key, 8, 600)
        .await
        .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "Повторите позже"))?;
    if !permitted {
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            "Слишком много попыток. Повторите позже.",
        ));
    }
    let hash = state
        .accounts
        .password_hash_for_user(user_id)
        .await
        .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "Повторите позже"))?
        .ok_or((StatusCode::UNAUTHORIZED, "Войдите снова"))?;
    let expected = hash.clone();
    let old = form.current_password;
    let new = form.new_password;
    let replacement = state
        .password_jobs
        .run(move || {
            if verify_password(&old, &hash) {
                hash_password(&new).ok()
            } else {
                None
            }
        })
        .await
        .map_err(|status| (status, "Повторите позже"))?
        .ok_or((
            StatusCode::UNAUTHORIZED,
            "Не удалось изменить пароль. Проверьте текущий пароль.",
        ))?;
    let token = generate_session_token();
    let updated = state
        .accounts
        .rotate_password_and_sessions(PasswordRotation {
            user_id,
            old_session: &session_token_digest(&session),
            expected_password_hash: &expected,
            new_password_hash: &replacement,
            new_session: &session_token_digest(&token),
            ttl: NonZeroU32::new(SESSION_AGE).unwrap(),
            audit,
        })
        .await
        .map_err(|_| (StatusCode::SERVICE_UNAVAILABLE, "Повторите позже"))?;
    if !updated {
        return Err((
            StatusCode::CONFLICT,
            "Сеанс или пароль уже изменён. Войдите снова.",
        ));
    }
    let _ = state.accounts.clear_auth_attempt(&key).await;
    Ok(token)
}

pub(super) async fn submit(
    State(state): State<AuthState>,
    headers: HeaderMap,
    peer: ConnectInfo<SocketAddr>,
    Form(form): Form<PasswordChangeForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return private_response(StatusCode::FORBIDDEN, &[]);
    }
    let actor = match super::session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return private_response(status, &[]),
    };
    let csrf = form.csrf_token.clone().unwrap_or_default();
    let audit = access_audit::context(&headers, peer.0, LoginEventKind::PasswordChange);
    match rotate(&state, &headers, actor, form, audit).await {
        Ok(token) => private_response(Redirect::to("/me"), &[state.cookies.session_cookie(&token)]),
        Err((status, message)) => private_response(
            (status, pages::password_change_form(&csrf, Some(message))),
            &[],
        ),
    }
}

pub(super) async fn api_submit(
    State(state): State<AuthState>,
    headers: HeaderMap,
    peer: ConnectInfo<SocketAddr>,
    Json(form): Json<ApiPasswordForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return super::profile_http::api_error(
            StatusCode::FORBIDDEN,
            "CSRF_REQUIRED",
            "Обновите страницу настроек",
        );
    }
    let actor = match super::session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => {
            return super::profile_http::api_error(
                StatusCode::UNAUTHORIZED,
                "AUTH_REQUIRED",
                "Войдите снова",
            )
        }
        Err(status) => {
            return super::profile_http::api_error(status, "UNAVAILABLE", "Повторите позже")
        }
    };
    let command = PasswordChangeForm {
        current_password: form.current_password,
        confirm_password: form.new_password.clone(),
        new_password: form.new_password,
        csrf_token: form.csrf_token,
    };
    let audit = access_audit::context(&headers, peer.0, LoginEventKind::PasswordChange);
    match rotate(&state, &headers, actor, command, audit).await {
        Ok(token) => private_response(
            StatusCode::NO_CONTENT,
            &[state.cookies.session_cookie(&token)],
        ),
        Err((status, message)) => super::profile_http::api_error(
            if status == StatusCode::BAD_REQUEST {
                StatusCode::UNPROCESSABLE_ENTITY
            } else {
                status
            },
            "PASSWORD_CHANGE_DENIED",
            message,
        ),
    }
}
