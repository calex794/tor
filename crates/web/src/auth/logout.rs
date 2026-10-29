//! Account logout remains CSRF-protected after the Layer picker split.
use super::{deny, AuthState, LogoutForm};
use crate::auth_cookies::private_response;
use axum::{
    extract::{Form, State},
    http::{HeaderMap, StatusCode},
    response::{Redirect, Response},
};
use tor_accounts::services::security::session_token_digest;

pub(super) async fn logout(
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
