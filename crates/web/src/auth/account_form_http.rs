//! Standalone account forms and error responses; browser-only, no SQL in HTTP.
use super::{access_audit, auth_observation, honeypot, login_limiter, AuthState};
use crate::{
    auth_cookies::{private_response, CookiePolicy, SESSION_AGE},
    observability, pages,
};
use axum::{
    extract::{ConnectInfo, Form, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::get,
    Router,
};
use serde::Deserialize;
use std::{net::SocketAddr, num::NonZeroU32};
use tor_accounts::{
    domain::access_audit::LoginEventKind,
    services::security::{
        generate_session_token, hash_password, session_token_digest, verify_password,
    },
    services::{login_identity::login_identity, validation as rules},
};
use tor_observability::services::auth::LoginOutcome;

mod login_admission;
mod return_target;
use return_target::auth_return_target;

pub(super) fn router() -> Router<AuthState> {
    Router::new()
        .route("/register", get(show_registration).post(register))
        .route("/login", get(show_login).post(login))
        .route("/assets/tor-auth-dialog.js", get(dialog_script))
}

#[derive(Deserialize)]
pub(super) struct RegisterForm {
    email: String,
    display_name: Option<String>,
    password: String,
    website: Option<String>,
    csrf_token: Option<String>,
    manual_invite_code: Option<String>,
    next: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct LoginForm {
    email: String,
    password: String,
    website: Option<String>,
    csrf_token: Option<String>,
    next: Option<String>,
}

/// A bounded local return target; the destination still verifies live access.
#[derive(Deserialize)]
pub(super) struct AuthReturnQuery {
    next: Option<String>,
}

fn dialog_requested(headers: &HeaderMap) -> bool {
    headers
        .get("x-tor-auth-dialog")
        .is_some_and(|value| value == "1")
}

// Switching modal forms must not invalidate a valid CSRF token in another
// tab. Standalone navigation retains its existing fresh-token behavior.
fn form_csrf(policy: &CookiePolicy, headers: &HeaderMap) -> (String, Vec<String>) {
    if dialog_requested(headers) {
        policy.csrf_for_page(headers)
    } else {
        let (token, cookie) = policy.csrf_token_cookie();
        (token, vec![cookie])
    }
}

pub(super) async fn show_registration(
    State(state): State<AuthState>,
    Query(query): Query<AuthReturnQuery>,
    headers: HeaderMap,
) -> Response {
    let (token, cookies) = form_csrf(&state.cookies, &headers);
    let next = auth_return_target(query.next.as_deref());
    private_response(
        pages::registration_form_with_next(&token, next.as_deref()),
        &cookies,
    )
}

pub(super) async fn show_login(
    State(state): State<AuthState>,
    Query(query): Query<AuthReturnQuery>,
    headers: HeaderMap,
) -> Response {
    let (token, cookies) = form_csrf(&state.cookies, &headers);
    let next = auth_return_target(query.next.as_deref());
    private_response(
        pages::login_form_with_next(&token, next.as_deref()),
        &cookies,
    )
}

/// The dialog uses the exact same POST endpoints and security checks as the
/// standalone forms. Only the successful response representation differs.
fn auth_success(headers: &HeaderMap, next: Option<&str>, cookies: &[String]) -> Response {
    if dialog_requested(headers) {
        let mut response = private_response(StatusCode::NO_CONTENT, cookies);
        response
            .headers_mut()
            .insert("x-tor-auth-dialog", HeaderValue::from_static("success"));
        response
    } else {
        private_response(Redirect::to(next.unwrap_or("/me")), cookies)
    }
}

pub(super) async fn dialog_script() -> impl IntoResponse {
    (
        [
            (
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "public, max-age=3600"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        include_str!("../../assets/tor-auth-dialog.js"),
    )
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
    let next = auth_return_target(form.next.as_deref());
    private_response(
        (
            StatusCode::BAD_REQUEST,
            pages::registration_form_with_error_and_next(
                form.csrf_token.as_deref().unwrap_or(""),
                form.display_name.as_deref().unwrap_or(""),
                &form.email,
                Some(reason),
                next.as_deref(),
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

fn spray_rate_error(seconds: u32) -> Response {
    let mut reply = auth_problem(
        StatusCode::TOO_MANY_REQUESTS,
        "Слишком много попыток входа",
        "Повторите попытку немного позже.",
        false,
    );
    reply.headers_mut().insert(
        header::RETRY_AFTER,
        HeaderValue::from_str(&seconds.to_string()).expect("numeric retry-after"),
    );
    reply
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
        "Неверный логин, адрес почты или пароль. Проверьте данные и попробуйте ещё раз.",
        false,
    )
}

pub(super) async fn register(
    State(state): State<AuthState>,
    headers: HeaderMap,
    peer: ConnectInfo<SocketAddr>,
    Form(form): Form<RegisterForm>,
) -> Response {
    let next = auth_return_target(form.next.as_deref());
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
    // Filled decoys are rejected before account lookup, Argon2 and any write.
    if honeypot::filled(form.website.as_deref()) {
        honeypot::record(honeypot::TrapKind::Register);
        return auth_problem(
            StatusCode::BAD_REQUEST,
            "Не удалось завершить регистрацию",
            "Откройте страницу регистрации заново и повторите отправку.",
            true,
        );
    }
    let account = match rules::registration_with_optional_name(
        &form.email,
        form.display_name.as_deref(),
        &form.password,
    ) {
        Ok(account) => account,
        Err(message) => {
            return private_response(
                (
                    StatusCode::BAD_REQUEST,
                    pages::registration_form_with_error_and_next(
                        form.csrf_token.as_deref().unwrap_or(""),
                        form.display_name.as_deref().unwrap_or(""),
                        &form.email,
                        Some(message),
                        next.as_deref(),
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
    let mail_source = super::registration_mail::capture(&account.email, audit.verified_ip);
    match state
        .accounts
        .register_with_audited_session_referral_mail(
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
            mail_source,
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
            auth_success(
                &headers,
                next.as_deref(),
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
    let next = auth_return_target(form.next.as_deref());
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
    // A filled hidden field is a signal, never evidence to ban an IP or user.
    if honeypot::filled(form.website.as_deref()) {
        honeypot::record(honeypot::TrapKind::Login);
        observed.report(auth_observation::Outcome::InvalidForm, None);
        return invalid_credentials();
    }
    let identity = match login_identity(&form.email, &form.password) {
        Ok(identity) => identity,
        Err(_) => {
            observed.report(auth_observation::Outcome::InvalidForm, None);
            return invalid_credentials();
        }
    };
    let audit = source;
    observed.set_identity(identity.as_str());
    // Charge the verified IP and submitted identifier before any account lookup.
    let submitted_key =
        match login_limiter::limit(&state, "login", identity.as_str(), &headers, peer).await {
            Ok(key) => key,
            Err(status) => return login_admission::rate_error(status, &mut observed),
        };
    let admission = match login_admission::admit(
        &state,
        &identity,
        submitted_key,
        audit,
        &mut observed,
    )
    .await
    {
        Ok(admission) => admission,
        Err(response) => return response,
    };
    let login_admission::Admission {
        stored,
        spray,
        identity_key,
        submitted_key,
    } = admission;
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
        if let Some(ref guard) = spray {
            if !login_limiter::spray_failure(&state, guard).await {
                eprintln!("TOR_AUTH_UNAVAILABLE stage=spray_record");
                if guard.enforcing() {
                    return auth_limit_error(StatusCode::SERVICE_UNAVAILABLE, false);
                }
            }
        }
        return invalid_credentials();
    }
    let Some(user) = stored else {
        observed.report(auth_observation::Outcome::UnknownAccount, None);
        return invalid_credentials();
    };
    if login_admission::clear(&state, identity_key, submitted_key)
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
        .insert_verified_session_for_login(
            user.id,
            &user.password_hash,
            &digest,
            ttl,
            audit,
            &identity,
        )
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
    // Only an explicitly allowed local destination is accepted; otherwise /me.
    observed.report(auth_observation::Outcome::Success, Some(user.id));
    observability::record_login_outcome(LoginOutcome::Success);
    auth_success(
        &headers,
        next.as_deref(),
        &[state.cookies.session_cookie(&token)],
    )
}

#[cfg(test)]
mod tests;
