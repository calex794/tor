//! Standalone account forms and error responses; browser-only, no SQL in HTTP.
use super::{login_limiter, AuthState};
use crate::{
    auth_cookies::{private_response, SESSION_AGE},
    pages,
};
use axum::{
    extract::{ConnectInfo, Form, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
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
    manual_invite_code: Option<String>,
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

fn auth_limit_error(status: StatusCode, registration: bool) -> Response {
    if status == StatusCode::TOO_MANY_REQUESTS {
        let title = if registration {
            "Слишком много попыток регистрации"
        } else {
            "Слишком много попыток входа"
        };
        let mut response = auth_problem(
            status,
            title,
            "Превышено число попыток. Повторите через 10 минут.",
            registration,
        );
        response
            .headers_mut()
            .insert(header::RETRY_AFTER, HeaderValue::from_static("600"));
        response
    } else {
        let title = if registration {
            "Регистрация временно недоступна"
        } else {
            "Вход временно недоступен"
        };
        auth_problem(
            status,
            title,
            "Попробуйте снова немного позже.",
            registration,
        )
    }
}

fn login_storage_error(stage: &'static str) -> Response {
    // Static stage only: do not log email, password, cookie or password hash.
    eprintln!("TOR_AUTH_LOGIN_UNAVAILABLE stage={stage}");
    auth_problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "Вход временно недоступен",
        "Попробуйте войти немного позже.",
        false,
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
    if let Err(status) =
        login_limiter::limit(&state, "register", &account.email, &headers, peer).await
    {
        return auth_limit_error(status, true);
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
    let manual_code = match tor_referrals::domain::manual_code(form.manual_invite_code.as_deref()) {
        Ok(code) => code,
        Err(_) => {
            return auth_problem(
                StatusCode::BAD_REQUEST,
                "Неверный код приглашения",
                "Проверьте код и повторите регистрацию.",
                true,
            )
        }
    };
    let guest_hash = state
        .cookies
        .guest_token(&headers)
        .map(|token| session_token_digest(&token));
    let token = generate_session_token();
    let digest = session_token_digest(&token);
    let ttl = NonZeroU32::new(SESSION_AGE).expect("nonzero session lifetime");
    match state
        .accounts
        .register_with_session_referral(
            &account.email,
            &account.display_name,
            &hashed,
            &digest,
            ttl,
            tor_accounts::repositories::postgres::RegistrationReferral {
                guest_hash: guest_hash.as_ref(),
                manual_code,
            },
        )
        .await
    {
        Ok(user_id) => {
            // The pending event is already committed with the account. A failure
            // here must not lose attribution or turn a valid signup into an error.
            let referrals = state.referrals.clone();
            tokio::spawn(async move {
                let _ = referrals.finalize_one(user_id).await;
            });
            private_response(Redirect::to("/me"), &[state.cookies.session_cookie(&token)])
        }
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
    let identity_key = match login_limiter::limit(&state, "login", &email, &headers, peer).await {
        Ok(key) => key,
        Err(status) => return auth_limit_error(status, false),
    };
    let stored = match state.accounts.find_user(&email).await {
        Ok(stored) => stored,
        Err(_) => return login_storage_error("lookup"),
    };
    let known_hash = stored.as_ref().map(|u| u.password_hash.clone());
    let password = form.password;
    let verified = match state
        .password_jobs
        .run(move || match known_hash {
            Some(hash) => verify_password(&password, &hash),
            None => {
                // Unknown accounts still perform costly work to limit timing leaks.
                let _ = hash_password(&password);
                false
            }
        })
        .await
    {
        Ok(valid) => valid,
        Err(StatusCode::TOO_MANY_REQUESTS) => {
            let mut response = auth_problem(
                StatusCode::TOO_MANY_REQUESTS,
                "Сервис входа перегружен",
                "Повторите попытку через несколько секунд.",
                false,
            );
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from_static("10"));
            return response;
        }
        Err(_) => return login_storage_error("password_worker"),
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
        return login_storage_error("clear_attempt");
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
        return login_storage_error("create_session");
    }
    // No next parameter is accepted: redirects always stay on this site.
    private_response(Redirect::to("/me"), &[state.cookies.session_cookie(&token)])
}

#[cfg(test)]
mod login_error_tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn too_many_login_attempts_has_clear_429_and_retry_after() {
        let reply = auth_limit_error(StatusCode::TOO_MANY_REQUESTS, false);
        assert_eq!(reply.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(reply.headers()[header::RETRY_AFTER], "600");
        let body = to_bytes(reply.into_body(), 65_536).await.expect("HTML");
        let html = String::from_utf8(body.to_vec()).expect("UTF-8");
        assert!(html.contains("Слишком много попыток входа"));
        assert!(html.contains("10 минут"));
        assert!(!html.contains("password_hash"));
    }

    #[tokio::test]
    async fn registration_rate_limit_is_also_honest() {
        let reply = auth_limit_error(StatusCode::TOO_MANY_REQUESTS, true);
        assert_eq!(reply.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(reply.headers()[header::RETRY_AFTER], "600");
    }

    #[tokio::test]
    async fn unavailable_storage_is_not_misreported_as_throttling() {
        let reply = auth_limit_error(StatusCode::SERVICE_UNAVAILABLE, false);
        assert_eq!(reply.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(reply.headers().get(header::RETRY_AFTER).is_none());
        let html = to_bytes(reply.into_body(), 65_536).await.expect("HTML");
        assert!(String::from_utf8_lossy(&html).contains("Вход временно недоступен"));
    }
}
