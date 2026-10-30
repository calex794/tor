//! Private, CSRF-protected future delivery preferences. No external send.
use super::{deny, pages, session_user, AuthState};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Form, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use serde::Deserialize;
use tor_reading::repositories::{DeliveryMode, PreferenceError};

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
    let mode = match state.notification_preferences.delivery_preference(actor).await {
        Ok(mode) => mode,
        Err(PreferenceError::Invalid) => return deny(StatusCode::BAD_REQUEST),
        Err(PreferenceError::Unavailable) => return deny(StatusCode::SERVICE_UNAVAILABLE),
    };
    let (csrf, cookies) = state.cookies.csrf_for_page(&headers);
    private_response(pages::delivery_preferences_page(mode, &csrf), &cookies)
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
