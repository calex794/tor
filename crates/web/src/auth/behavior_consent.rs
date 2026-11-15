//! Cached behavior-consent commands retain authentication and CSRF guards.
use super::{deny, session_user, AuthState};
use axum::{
    extract::{Form, State},
    http::{HeaderMap, StatusCode},
    response::Response,
};
use serde::Deserialize;

#[derive(Deserialize)]
pub(super) struct BehaviorConsentForm {
    csrf_token: Option<String>,
    choice: String,
}

pub(super) async fn set_behavior_consent(
    State(state): State<AuthState>,
    headers: HeaderMap,
    Form(form): Form<BehaviorConsentForm>,
) -> Response {
    if !state.cookies.same_origin(&headers)
        || !state
            .cookies
            .valid_csrf(&headers, form.csrf_token.as_deref().unwrap_or(""))
    {
        return deny(StatusCode::FORBIDDEN);
    }
    if !matches!(form.choice.as_str(), "enable" | "disable") {
        return deny(StatusCode::BAD_REQUEST);
    }
    match session_user(&state, &headers).await {
        Ok(Some(_)) => crate::recommendation_http::retired(),
        Ok(None) => deny(StatusCode::UNAUTHORIZED),
        Err(status) => deny(status),
    }
}
