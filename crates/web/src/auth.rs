//! Real browser account flows, backed by PostgreSQL and revocable sessions.
#![forbid(unsafe_code)]

use crate::{
    auth_cookies::{private_response, CookiePolicy, SESSION_AGE},
    pages,
    password_jobs::PasswordJobs,
};
use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, Form, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use std::{net::SocketAddr, num::NonZeroU32};
use tor_accounts::{
    repositories::PgAccounts,
    services::security::{
        generate_session_token, hash_password, session_token_digest, verify_password,
    },
    services::validation as rules,
};

#[derive(Clone)]
struct AuthState {
    accounts: PgAccounts,
    cookies: CookiePolicy,
    password_jobs: PasswordJobs,
}

#[derive(Deserialize)]
struct RegisterForm {
    email: String,
    display_name: String,
    password: String,
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
struct LoginForm {
    email: String,
    password: String,
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
struct LogoutForm {
    csrf_token: Option<String>,
}

pub(crate) fn router(accounts: PgAccounts, secure: bool, origin: String) -> Router {
    let state = AuthState {
        accounts,
        cookies: CookiePolicy { secure, origin },
        password_jobs: PasswordJobs::new(4),
    };
    Router::new()
        .route("/register", get(show_registration).post(register))
        .route("/login", get(show_login).post(login))
        .route("/me", get(me))
        .route("/logout", post(logout))
        .layer(DefaultBodyLimit::max(4096))
        .with_state(state)
}

async fn show_registration(State(state): State<AuthState>) -> Response {
    let (token, cookie) = state.cookies.csrf_token_cookie();
    private_response(pages::registration_form(&token), &[cookie])
}

async fn show_login(State(state): State<AuthState>) -> Response {
    let (token, cookie) = state.cookies.csrf_token_cookie();
    private_response(pages::login_form(&token), &[cookie])
}

fn deny(status: StatusCode) -> Response {
    private_response(status, &[])
}

fn invalid_credentials() -> Response {
    private_response(
        (StatusCode::UNAUTHORIZED, "Неверный адрес почты или пароль"),
        &[],
    )
}

/// Reserve capacity before Argon2 to slow password guessing and account spray.
/// A client IP comes from the TCP peer, NEVER from untrusted forwarding headers.
async fn limit(
    state: &AuthState,
    action: &str,
    email: &str,
    peer: ConnectInfo<SocketAddr>,
) -> Result<[u8; 32], StatusCode> {
    let identity_key = session_token_digest(&format!("auth:{action}:identity:{email}"));
    let ip = peer.0.ip().to_string();
    let address_key = session_token_digest(&format!("auth:{action}:peer:{ip}"));
    // No in-process counters: these limits apply across all server processes.
    let address_ok = state
        .accounts
        .permit_auth_attempt(&address_key, 60, 600)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if !address_ok {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    let identity_ok = state
        .accounts
        .permit_auth_attempt(&identity_key, 8, 600)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if !identity_ok {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    Ok(identity_key)
}

async fn register(
    State(state): State<AuthState>,
    headers: HeaderMap,
    peer: ConnectInfo<SocketAddr>,
    Form(form): Form<RegisterForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let account = match rules::registration(&form.email, &form.display_name, &form.password) {
        Ok(account) => account,
        Err(_) => return deny(StatusCode::BAD_REQUEST),
    };
    if let Err(status) = limit(&state, "register", &account.email, peer).await {
        return deny(status);
    }

    // Argon2 is deliberately moved off Tokio's request executor.
    let password = form.password;
    let hashed = match state
        .password_jobs
        .run(move || hash_password(&password))
        .await
    {
        Ok(Ok(hash)) => hash,
        Ok(Err(_)) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        Err(status) => return deny(status),
    };
    let token = generate_session_token();
    let digest = session_token_digest(&token);
    let ttl = NonZeroU32::new(SESSION_AGE).expect("nonzero session lifetime");
    match state
        .accounts
        .register_with_session(&account.email, &account.display_name, &hashed, &digest, ttl)
        .await
    {
        Ok(_) => private_response(Redirect::to("/me"), &[state.cookies.session_cookie(&token)]),
        Err(tor_accounts::repositories::postgres::AccountWriteError::Duplicate) => {
            // No user ID or database exception is disclosed.
            private_response(
                (StatusCode::CONFLICT, "Не удалось создать учётную запись"),
                &[],
            )
        }
        Err(_) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

async fn login(
    State(state): State<AuthState>,
    headers: HeaderMap,
    peer: ConnectInfo<SocketAddr>,
    Form(form): Form<LoginForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let email = match rules::login(&form.email, &form.password) {
        Ok(email) => email,
        Err(_) => return invalid_credentials(),
    };
    let identity_key = match limit(&state, "login", &email, peer).await {
        Ok(key) => key,
        Err(status) => return deny(status),
    };
    let stored = match state.accounts.find_user(&email).await {
        Ok(stored) => stored,
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let known_hash = stored.as_ref().map(|u| u.password_hash.clone());
    let password = form.password;
    let verified = match state
        .password_jobs
        .run(move || match known_hash {
            Some(hash) => verify_password(&password, &hash),
            None => {
                // A missing account still performs costly work to limit timing leaks.
                let _ = hash_password(&password);
                false
            }
        })
        .await
    {
        Ok(valid) => valid,
        Err(status) => return deny(status),
    };
    if !verified {
        return invalid_credentials();
    }
    let Some(user) = stored else {
        return invalid_credentials();
    };

    if state
        .accounts
        .clear_auth_attempt(&identity_key)
        .await
        .is_err()
    {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    }
    let token = generate_session_token();
    let digest = session_token_digest(&token);
    let ttl = NonZeroU32::new(SESSION_AGE).expect("nonzero session lifetime");
    if state
        .accounts
        .insert_session(user.id, &digest, ttl)
        .await
        .is_err()
    {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    }
    // No next parameter is accepted: redirects always stay on this site.
    private_response(Redirect::to("/me"), &[state.cookies.session_cookie(&token)])
}

async fn me(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let Some(token) = state.cookies.session_token(&headers) else {
        return private_response(Redirect::to("/login"), &[]);
    };
    let digest = session_token_digest(&token);
    let user_id = match state.accounts.active_session_user(&digest).await {
        Ok(Some(id)) => id,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let name = match state.accounts.user_display_name(user_id).await {
        Ok(Some(value)) => value,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(_) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(pages::my_account(&name, &csrf), &cookies)
}

async fn logout(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<LogoutForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    if let Some(token) = state.cookies.session_token(&headers) {
        let digest = session_token_digest(&token);
        if state.accounts.revoke_session(&digest).await.is_err() {
            return deny(StatusCode::SERVICE_UNAVAILABLE);
        }
    }
    private_response(Redirect::to("/login"), &state.cookies.cleared_cookies())
}
