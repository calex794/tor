//! A-07: generic request acknowledgement and single-use proof confirmation.
use super::{login_limiter, AuthState};
use crate::{auth_cookies::private_response, pages};
use axum::{
    extract::{ConnectInfo, Form, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use std::net::SocketAddr;
use tor_accounts::{
    domain::normalize_email,
    services::{
        security::{generate_session_token, hash_password, session_token_digest, verify_password},
        validation::password_reset,
    },
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RequestForm {
    email: String,
    csrf_token: Option<String>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ConfirmForm {
    token: String,
    new_password: String,
    confirm_password: String,
    csrf_token: Option<String>,
}

pub(super) async fn request_page(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(
        pages::reset_request_form(&csrf, state.reset_delivery.is_some(), None),
        &cookies,
    )
}

pub(super) async fn request(
    State(state): State<AuthState>,
    peer: ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<RequestForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return private_response(StatusCode::FORBIDDEN, &[]);
    }
    let Some(delivery) = state.reset_delivery.clone() else {
        return private_response(StatusCode::SERVICE_UNAVAILABLE, &[]);
    };
    let email = match normalize_email(&form.email) {
        Ok(email) => email,
        Err(_) => return private_response(StatusCode::UNPROCESSABLE_ENTITY, &[]),
    };
    if let Err(status) = login_limiter::limit(&state, "reset-request", &email, &headers, peer).await
    {
        return private_response(status, &[]);
    }
    let Some(permit) = delivery.reserve() else {
        return private_response(StatusCode::TOO_MANY_REQUESTS, &[]);
    };
    // All account lookup and delivery happen after the identical HTTP response path.
    // A bounded slot is acquired for known and unknown identities alike.
    let accounts = state.accounts.clone();
    tokio::spawn(async move {
        if let Ok(Some(user)) = accounts.find_user(&email).await {
            let token = generate_session_token();
            if matches!(
                accounts
                    .issue_password_reset(
                        user.id,
                        &user.password_hash,
                        &session_token_digest(&token)
                    )
                    .await,
                Ok(true)
            ) && !delivery.send(user.email_normalized, token, permit).await
            {
                eprintln!("TOR_RESET_DELIVERY_UNAVAILABLE");
            }
        }
    });
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(pages::reset_request_form(&csrf, true, Some("Если учётная запись существует, на её адрес будет отправлена ссылка для смены пароля.")), &cookies)
}

// Only the reset document enables reviewed same-origin JS; inline JS stays forbidden.
fn confirm_response(result: impl IntoResponse, cookies: &[String]) -> Response {
    let mut response = private_response(result, cookies);
    response.headers_mut().insert("content-security-policy", HeaderValue::from_static(
        "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self'; form-action 'self'; base-uri 'none'; frame-ancestors 'none'",
    ));
    response
}

pub(super) async fn script() -> Response {
    private_response(
        (
            [(
                header::CONTENT_TYPE,
                "application/javascript; charset=utf-8",
            )],
            include_str!("../../assets/account-reset.js"),
        ),
        &[],
    )
}

pub(super) async fn confirm_page(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    confirm_response(pages::reset_confirm_form(&csrf, None), &cookies)
}

pub(super) async fn confirm(
    State(state): State<AuthState>,
    peer: ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Form(form): Form<ConfirmForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return confirm_response(StatusCode::FORBIDDEN, &[]);
    }
    if state.reset_delivery.is_none() {
        return confirm_response(StatusCode::SERVICE_UNAVAILABLE, &[]);
    }
    // Apply the same independent identity/IP limits to known, expired and forged proofs.
    if let Err(status) =
        login_limiter::limit(&state, "reset-confirm", &form.token, &headers, peer).await
    {
        return confirm_response(status, &[]);
    }
    let csrf = form.csrf_token.as_deref().unwrap_or("");
    if form.token.len() != 43
        || !form
            .token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return confirm_response(
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                pages::reset_confirm_form(
                    csrf,
                    Some("Ссылка недействительна или устарела. Запросите новую."),
                ),
            ),
            &[],
        );
    }
    if let Err(message) = password_reset(&form.new_password, &form.confirm_password) {
        return confirm_response(
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                pages::reset_confirm_form(csrf, Some(message)),
            ),
            &[],
        );
    }
    let stored_hash = match state
        .accounts
        .live_reset_password_hash(&session_token_digest(&form.token))
        .await
    {
        Ok(Some(hash)) => hash,
        Ok(None) => {
            return confirm_response(
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    pages::reset_confirm_form(
                        csrf,
                        Some("Ссылка недействительна или устарела. Запросите новую."),
                    ),
                ),
                &[],
            )
        }
        Err(_) => return confirm_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    };
    let password = form.new_password;
    let hash = match state
        .password_jobs
        .run(move || {
            if verify_password(&password, &stored_hash) {
                return Ok(None);
            }
            hash_password(&password).map(Some)
        })
        .await
    {
        Ok(Ok(Some(hash))) => hash,
        Ok(Ok(None)) => {
            return confirm_response(
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    pages::reset_confirm_form(
                        csrf,
                        Some("Новый пароль должен отличаться от текущего"),
                    ),
                ),
                &[],
            )
        }
        Ok(Err(_)) => return confirm_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
        Err(status) => return confirm_response(status, &[]),
    };
    match state
        .accounts
        .consume_password_reset(&session_token_digest(&form.token), &hash)
        .await
    {
        Ok(true) => confirm_response(Redirect::to("/login"), &state.cookies.cleared_cookies()),
        Ok(false) => confirm_response(
            (
                StatusCode::UNPROCESSABLE_ENTITY,
                pages::reset_confirm_form(
                    csrf,
                    Some("Ссылка недействительна или устарела. Запросите новую."),
                ),
            ),
            &[],
        ),
        Err(_) => confirm_response(StatusCode::SERVICE_UNAVAILABLE, &[]),
    }
}
