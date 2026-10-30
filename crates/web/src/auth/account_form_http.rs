//! Standalone account forms and error responses; browser-only, no SQL in HTTP.
use super::{access_audit, auth_observation, login_limiter, AuthState};
use crate::{
    auth_cookies::{private_response, SESSION_AGE},
    observability, pages,
};
use axum::{
    extract::{ConnectInfo, Form, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use std::{net::SocketAddr, num::NonZeroU32};
use tor_accounts::{
    domain::access_audit::LoginEventKind,
    services::security::{
        generate_session_token, hash_password, session_token_digest, verify_password,
    },
    services::validation as rules,
};
use tor_observability::services::auth::LoginOutcome;

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

fn invalid_invite(form: &RegisterForm, reason: &str) -> Response {
    // A failed manual code never changes the existing guest referral capture.
    private_response(
        (
            StatusCode::BAD_REQUEST,
            pages::registration_form_with_error(
                form.csrf_token.as_deref().unwrap_or(""),
                &form.display_name,
                &form.email,
                Some(reason),
            ),
        ),
        &[],
    )
}

fn auth_limit_error(status: StatusCode, registration: bool) -> Response {
    if !registration {
        observability::record_login_outcome(if status == StatusCode::TOO_MANY_REQUESTS {
            LoginOutcome::RateLimited
        } else {
            LoginOutcome::RateLimiterUnavailable
        });
    }
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

fn login_storage_error(stage: LoginOutcome) -> Response {
    observability::record_login_outcome(stage);
    // Static label only: do not log email, password, cookie or password hash.
    eprintln!("TOR_AUTH_LOGIN_UNAVAILABLE stage={}", stage.label());
    auth_problem(
        StatusCode::SERVICE_UNAVAILABLE,
        "Вход временно недоступен",
        "Попробуйте войти немного позже.",
        false,
    )
}

fn invalid_credentials() -> Response {
    observability::record_login_outcome(LoginOutcome::InvalidCredentials);
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
    let audit = access_audit::context(&headers, peer.0, LoginEventKind::Registration);
    if let Err(status) =
        login_limiter::limit(&state, "register", &account.email, &headers, peer).await
    {
        return auth_limit_error(status, true);
    }

    // Check the manual invitation before Argon2 and before account creation.
    // Invalid or disabled codes must not silently result in a new account.
    let manual_code = match tor_referrals::domain::manual_code(form.manual_invite_code.as_deref()) {
        Ok(code) => code,
        Err(_) => {
            return invalid_invite(
                &form,
                "Некорректный код приглашения. Исправьте или удалите код.",
            )
        }
    };
    if let Some(code) = manual_code {
        match state.referrals.active_code(code).await {
            Ok(true) => {}
            Ok(false) => return invalid_invite(
                &form,
                "Код приглашения не найден или больше не действует. Уберите код, чтобы продолжить.",
            ),
            Err(_) => {
                return auth_problem(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Регистрация временно недоступна",
                    "Не удалось проверить код приглашения. Попробуйте снова позже.",
                    true,
                )
            }
        }
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
    let guest_hash = state
        .cookies
        .guest_token(&headers)
        .map(|token| session_token_digest(&token));
    let token = generate_session_token();
    let digest = session_token_digest(&token);
    let ttl = NonZeroU32::new(SESSION_AGE).expect("nonzero session lifetime");
    match state
        .accounts
        .register_with_audited_session_referral(
            &account.email,
            &account.display_name,
            &hashed,
            &digest,
            ttl,
            tor_accounts::repositories::postgres::RegistrationContext {
                referral: tor_accounts::repositories::postgres::RegistrationReferral {
                    guest_hash: guest_hash.as_ref(),
                    manual_code,
                },
                audit,
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
            private_response(
                Redirect::to("/me"),
                &[
                    state.cookies.session_cookie(&token),
                    state.cookies.cleared_guest_cookie(),
                ],
            )
        }
        Err(tor_accounts::repositories::postgres::AccountWriteError::InvalidReferral) => {
            auth_problem(
                StatusCode::BAD_REQUEST,
                "Код приглашения больше не действует",
                "Проверьте код приглашения и повторите регистрацию.",
                true,
            )
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
    let source = access_audit::context(&headers, peer.0, LoginEventKind::PasswordLogin);
    let mut observed = auth_observation::RequestObservation::new(&state, &form.email, source);
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        observed.report(auth_observation::Outcome::InvalidForm, None);
        return auth_problem(
            StatusCode::FORBIDDEN,
            "Форма устарела",
            "Откройте страницу входа заново и повторите отправку.",
            false,
        );
    }
    let email = match rules::login(&form.email, &form.password) {
        Ok(email) => email,
        Err(_) => {
            observed.report(auth_observation::Outcome::InvalidForm, None);
            return invalid_credentials();
        }
    };
    let audit = source;
    let identity_key = match login_limiter::limit(&state, "login", &email, &headers, peer).await {
        Ok(key) => key,
        Err(status) => {
            observed.report(
                if status == StatusCode::TOO_MANY_REQUESTS {
                    auth_observation::Outcome::RateLimited
                } else {
                    auth_observation::Outcome::Unavailable
                },
                None,
            );
            return auth_limit_error(status, false);
        }
    };
    let stored = match state.accounts.find_user(&email).await {
        Ok(stored) => stored,
        Err(_) => {
            observed.report(auth_observation::Outcome::Unavailable, None);
            return login_storage_error(LoginOutcome::LookupUnavailable);
        }
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
            observed.report(
                auth_observation::Outcome::WorkerBusy,
                stored.as_ref().map(|u| u.id),
            );
            observability::record_login_outcome(LoginOutcome::PasswordPoolBusy);
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
        Err(_) => {
            observed.report(
                auth_observation::Outcome::Unavailable,
                stored.as_ref().map(|u| u.id),
            );
            return login_storage_error(LoginOutcome::PasswordWorkerUnavailable);
        }
    };
    if !verified {
        observed.report(
            if stored.is_some() {
                auth_observation::Outcome::BadPassword
            } else {
                auth_observation::Outcome::UnknownAccount
            },
            stored.as_ref().map(|u| u.id),
        );
        return invalid_credentials();
    }
    let Some(user) = stored else {
        observed.report(auth_observation::Outcome::UnknownAccount, None);
        return invalid_credentials();
    };
    if state
        .accounts
        .clear_auth_attempt(&identity_key)
        .await
        .is_err()
    {
        observed.report(auth_observation::Outcome::Unavailable, Some(user.id));
        return login_storage_error(LoginOutcome::AttemptResetUnavailable);
    }
    let token = generate_session_token();
    let digest = session_token_digest(&token);
    let ttl = NonZeroU32::new(SESSION_AGE).expect("nonzero session lifetime");
    match state
        .accounts
        .insert_verified_session(user.id, &user.password_hash, &digest, ttl, audit)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            observed.report(auth_observation::Outcome::Unavailable, Some(user.id));
            return invalid_credentials();
        }
        Err(_) => {
            observed.report(auth_observation::Outcome::Unavailable, Some(user.id));
            return login_storage_error(LoginOutcome::SessionUnavailable);
        }
    }
    // No next parameter is accepted: redirects always stay on this site.
    observed.report(auth_observation::Outcome::Success, Some(user.id));
    observability::record_login_outcome(LoginOutcome::Success);
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
