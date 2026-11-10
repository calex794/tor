//! Private, CSRF-protected future delivery preferences. No external send.
use super::{deny, pages, session_user, AuthState};
use crate::auth_cookies::{private_response, valid_token};
use axum::{
    extract::{Form, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_accounts::services::security::{generate_session_token, session_token_digest};
use tor_reading::repositories::{
    DeliveryMode, PgEmailNotificationConsent, PgPreferredSendHour, PreferenceError,
};

#[derive(Deserialize)]
pub(super) struct DeliveryForm {
    csrf_token: Option<String>,
    delivery_mode: String,
}

pub(super) async fn page(State(state): State<AuthState>, headers: HeaderMap) -> Response {
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return private_response(Redirect::to("/login"), &[]),
        Err(status) => return deny(status),
    };
    let mode = match state
        .notification_preferences
        .delivery_preference(actor)
        .await
    {
        Ok(mode) => mode,
        Err(PreferenceError::Invalid) => return deny(StatusCode::BAD_REQUEST),
        Err(PreferenceError::Unavailable) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let smart_hour = if mode == DeliveryMode::SmartTime {
        match PgPreferredSendHour::new(state.accounts.pool().clone())
            .estimate(actor)
            .await
        {
            Ok(hour) => Some(hour),
            Err(PreferenceError::Invalid) => return deny(StatusCode::BAD_REQUEST),
            Err(PreferenceError::Unavailable) => return deny(StatusCode::SERVICE_UNAVAILABLE),
        }
    } else {
        None
    };
    let opted_in = match PgEmailNotificationConsent::new(state.accounts.pool().clone())
        .opted_in(actor)
        .await
    {
        Ok(value) => value,
        Err(PreferenceError::Invalid) => return deny(StatusCode::BAD_REQUEST),
        Err(PreferenceError::Unavailable) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(
        pages::delivery_preferences_page(
            mode,
            &csrf,
            smart_hour,
            opted_in,
            state.notification_proof_delivery.is_some(),
        ),
        &cookies,
    )
}

pub(super) async fn save(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<DeliveryForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !form
            .csrf_token
            .as_deref()
            .is_some_and(|csrf| state.cookies.valid_csrf(&headers, csrf))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    let Some(mode) = DeliveryMode::parse(&form.delivery_mode) else {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    };
    match state
        .notification_preferences
        .set_delivery_preference(actor, mode)
        .await
    {
        Ok(()) => private_response(Redirect::to("/me/notification-settings"), &[]),
        Err(PreferenceError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(PreferenceError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProofRequestForm {
    csrf_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProofConfirmForm {
    csrf_token: Option<String>,
    proof_code: String,
    email_opt_in: Option<String>,
}

pub(super) async fn request_email_proof(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<ProofRequestForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !form
            .csrf_token
            .as_deref()
            .is_some_and(|csrf| state.cookies.valid_csrf(&headers, csrf))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    // A password-reset mailer is NEVER substituted for the separate
    // explicitly configured content-verification transport.
    let Some(sender) = state.notification_proof_delivery.clone() else {
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    };
    let Some(permit) = sender.reserve() else {
        return deny(StatusCode::TOO_MANY_REQUESTS);
    };
    let consent = PgEmailNotificationConsent::new(state.accounts.pool().clone());
    let email = match consent.mailbox_for_proof(actor).await {
        Ok(Some(email)) => email,
        Ok(None) => return deny(StatusCode::NOT_FOUND),
        Err(PreferenceError::Invalid) => return deny(StatusCode::BAD_REQUEST),
        Err(PreferenceError::Unavailable) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    if !sender.mailbox_allowed(&email) {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let token = generate_session_token();
    let digest = session_token_digest(&token);
    match consent.issue_proof(actor, &digest).await {
        Ok(true) => {}
        Ok(false) => return deny(StatusCode::TOO_MANY_REQUESTS),
        Err(PreferenceError::Invalid) => return deny(StatusCode::BAD_REQUEST),
        Err(PreferenceError::Unavailable) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    }
    if !sender.send(email, token, permit).await {
        // Never echo a token, mailbox or transport diagnostics to the caller.
        return deny(StatusCode::SERVICE_UNAVAILABLE);
    }
    private_response(Redirect::to("/me/notification-settings"), &[])
}

pub(super) async fn confirm_email_proof(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<ProofConfirmForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !form
            .csrf_token
            .as_deref()
            .is_some_and(|csrf| state.cookies.valid_csrf(&headers, csrf))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    // Email proof and explicit content opt-in are TWO independent signals.
    if !valid_token(&form.proof_code) || form.email_opt_in.as_deref() != Some("on") {
        return deny(StatusCode::UNPROCESSABLE_ENTITY);
    }
    let digest = session_token_digest(&form.proof_code);
    match PgEmailNotificationConsent::new(state.accounts.pool().clone())
        .confirm_opt_in(actor, &digest)
        .await
    {
        Ok(true) => private_response(Redirect::to("/me/notification-settings"), &[]),
        Ok(false) => deny(StatusCode::UNPROCESSABLE_ENTITY),
        Err(PreferenceError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(PreferenceError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}

pub(super) async fn revoke_email_consent(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<ProofRequestForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !form
            .csrf_token
            .as_deref()
            .is_some_and(|csrf| state.cookies.valid_csrf(&headers, csrf))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    let actor = match session_user(&state, &headers).await {
        Ok(Some(actor)) => actor,
        Ok(None) => return deny(StatusCode::UNAUTHORIZED),
        Err(status) => return deny(status),
    };
    match PgEmailNotificationConsent::new(state.accounts.pool().clone())
        .revoke(actor)
        .await
    {
        Ok(_) => private_response(Redirect::to("/me/notification-settings"), &[]),
        Err(PreferenceError::Invalid) => deny(StatusCode::BAD_REQUEST),
        Err(PreferenceError::Unavailable) => deny(StatusCode::SERVICE_UNAVAILABLE),
    }
}
