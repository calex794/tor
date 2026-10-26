//! Standalone account forms and error responses; browser-only, no SQL in HTTP.
use super::{login_limiter, AuthState};
use crate::{
    auth_cookies::{private_response, SESSION_AGE},
    pages,
};
use axum::{
    extract::{ConnectInfo, Form, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use std::{net::SocketAddr, num::NonZeroU32};
use tor_accounts::{
    services::security::{
        generate_session_token, hash_password, session_token_digest, verify_password,
    },
    services::validation as rules,
};

#[derive(Deserialize)]
pub(super) struct RegisterForm {
    email: String,
    display_name: String,
    password: String,
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct LoginForm {
    email: String,
    password: String,
    csrf_token: Option<String>,
}

pub(super) async fn show_registration(State(state): State<AuthState>) -> Response {
    let (token, cookie) = state.cookies.csrf_token_cookie();
    private_response(pages::registration_form(&token), &[cookie])
}

pub(super) async fn show_login(State(state): State<AuthState>) -> Response {
    let (token, cookie) = state.cookies.csrf_token_cookie();
    private_response(pages::login_form(&token), &[cookie])
}

fn auth_problem(status: StatusCode, title: &str, message: &str, registration: bool) -> Response {
    private_response(
        (
            status,
            pages::account_error_page(title, message, registration),
        ),
        &[],
    )
}

fn invalid_credentials() -> Response {
    auth_problem(
        StatusCode::UNAUTHORIZED,
        "Не удалось войти",
        "Неверный адрес почты или пароль. Проверьте данные и попробуйте ещё раз.",
        false,
    )
}

pub(super) async fn register(
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
        return auth_problem(
            StatusCode::FORBIDDEN,
            "Форма устарела",
            "Откройте регистрацию заново и повторите отправку.",
            true,
        );
    }
    let account = match rules::registration(&form.email, &form.display_name, &form.password) {
        Ok(account) => account,
        Err(message) => {
            return private_response(
                (
                    StatusCode::BAD_REQUEST,
                    pages::registration_form_with_error(
                        form.csrf_token.as_deref().unwrap_or(""),
                        &form.display_name,
                        &form.email,
                        Some(message),
                    ),
                ),
                &[],
            );
        }
    };
    if let Err(status) = login_limiter::limit(&state, "register", &account.email, peer).await {
        return auth_problem(
            status,
            "Регистрация временно недоступна",
            "Попробуйте снова чуть позже.",
            true,
        );
    }

    // Argon2 is deliberately moved off Tokio's request executor.
    let password = form.password;
    let hashed = match state
        .password_jobs
        .run(move || hash_password(&password))
        .await
    {
        Ok(Ok(hash)) => hash,
        Ok(Err(_)) => {
            return auth_problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "Не удалось завершить регистрацию",
                "Попробуйте повторить регистрацию немного позже.",
                true,
            );
        }
        Err(status) => {
            return auth_problem(
                status,
                "Не удалось завершить регистрацию",
                "Попробуйте повторить регистрацию немного позже.",
                true,
            );
        }
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
            // No account ID, email or database exception is disclosed.
            auth_problem(
                StatusCode::CONFLICT,
                "Не удалось завершить регистрацию",
                "Проверьте адрес почты. Если вы уже регистрировались, попробуйте войти.",
                true,
            )
        }
        Err(_) => auth_problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "Регистрация временно недоступна",
            "Попробуйте снова немного позже.",
            true,
        ),
    }
}

pub(super) async fn login(
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
        return auth_problem(
            StatusCode::FORBIDDEN,
            "Форма устарела",
            "Откройте страницу входа заново и повторите отправку.",
            false,
        );
    }
    let email = match rules::login(&form.email, &form.password) {
        Ok(email) => email,
        Err(_) => return invalid_credentials(),
    };
    let identity_key = match login_limiter::limit(&state, "login", &email, peer).await {
        Ok(key) => key,
        Err(status) => {
            return auth_problem(
                status,
                "Вход временно недоступен",
                "Попробуйте войти немного позже.",
                false,
            );
        }
    };
    let stored = match state.accounts.find_user(&email).await {
        Ok(stored) => stored,
        Err(_) => {
            return auth_problem(
                StatusCode::SERVICE_UNAVAILABLE,
                "Вход временно недоступен",
                "Попробуйте войти немного позже.",
                false,
            );
        }
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
        Err(status) => {
            return auth_problem(
                status,
                "Вход временно недоступен",
                "Попробуйте войти немного позже.",
                false,
            );
        }
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
        return auth_problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "Вход временно недоступен",
            "Попробуйте войти немного позже.",
            false,
        );
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
        return auth_problem(
            StatusCode::SERVICE_UNAVAILABLE,
            "Вход временно недоступен",
            "Попробуйте войти немного позже.",
            false,
        );
    }
    // No next parameter is accepted: redirects always stay on this site.
    private_response(Redirect::to("/me"), &[state.cookies.session_cookie(&token)])
}
