//! A-03: browser-only session ownership and explicit revocation.
use super::AuthState;
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{Form, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_accounts::services::security::session_token_digest;
use tor_accounts::services::security::verify_password;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RevokeForm {
    session_id: String,
    current_password: String,
    csrf_token: Option<String>,
}

pub(super) async fn page(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let user = match super::session_user(&state, &headers).await {
        Ok(Some(user)) => user,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return private_response(status, &[]),
    };
    let Some(token) = state.cookies.session_token(&headers) else {
        return private_response(StatusCode::UNAUTHORIZED, &[]);
    };
    let digest = session_token_digest(&token);
    let sessions = match state.accounts.list_active_sessions(user, &digest).await {
        Ok(sessions) => sessions,
        Err(_) => return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(pages::active_sessions(&csrf, &sessions), &cookies)
}

pub(super) async fn revoke(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<RevokeForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return private_response(StatusCode::FORBIDDEN, &[]);
    }
    let user = match super::session_user(&state, &headers).await {
        Ok(Some(user)) => user,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return private_response(status, &[]),
    };
    if !valid_session_id(&form.session_id) {
        return private_response(StatusCode::BAD_REQUEST, &[]);
    }
    let Some(token) = state.cookies.session_token(&headers) else {
        return private_response(StatusCode::UNAUTHORIZED, &[]);
    };
    if form.current_password.is_empty() || form.current_password.len() > 512 {
        return private_response(StatusCode::BAD_REQUEST, &[]);
    }
    let attempts = session_token_digest(&format!("auth:revoke-session:{user}"));
    match state.accounts.permit_auth_attempt(&attempts, 8, 600).await {
        Ok(true) => {}
        Ok(false) => return private_response(StatusCode::TOO_MANY_REQUESTS, &[]),
        Err(_) => return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    }
    let expected = match state.accounts.password_hash_for_user(user).await {
        Ok(Some(hash)) => hash,
        _ => return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    };
    let password = form.current_password;
    let verify_hash = expected.clone();
    let valid = match state
        .password_jobs
        .run(move || verify_password(&password, &verify_hash))
        .await
    {
        Ok(valid) => valid,
        Err(status) => return private_response(status, &[]),
    };
    if !valid {
        return private_response(StatusCode::UNAUTHORIZED, &[]);
    }
    match state
        .accounts
        .revoke_owned_session(
            user,
            &session_token_digest(&token),
            &form.session_id,
            &expected,
        )
        .await
    {
        Ok(Some(true)) => {
            private_response(Redirect::to("/login"), &state.cookies.cleared_cookies())
        }
        Ok(Some(false)) => private_response(Redirect::to("/me/security/sessions"), &[]),
        Ok(None) => private_response(StatusCode::NOT_FOUND, &[]),
        Err(_) => private_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    }
}

fn valid_session_id(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}

#[cfg(test)]
mod tests {
    use super::valid_session_id;

    #[test]
    fn a_03_rejects_invalid_session_identifiers() {
        assert!(valid_session_id("3f6bce1a-65d6-4c3f-9001-2390c70cf711"));
        for bad in [
            "not-a-uuid",
            "3f6bce1a-65d6-4c3f-9001-2390c70cf711 ",
            "3f6bce1a-65d6-4c3f-9001-2390c70cf71z",
            "3f6bce1a-65d6-4c3f-9001-2390c70cf711' OR true",
        ] {
            assert!(!valid_session_id(bad));
        }
    }
}
