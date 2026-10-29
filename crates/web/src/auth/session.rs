//! The sole shared resolver of a revocable, server-authenticated session.
use super::AuthState;
use axum::http::{HeaderMap, StatusCode};
use tor_accounts::services::security::session_token_digest;

/// Resolve actor from the session database, never client-supplied user IDs.
pub(super) async fn session_user(state: &AuthState, headers: &HeaderMap) -> Result<Option<i64>, StatusCode> {
    let Some(token) = state.cookies.session_token(headers) else {
        return Ok(None);
    };
    state
        .accounts
        .active_session_user(&session_token_digest(&token))
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)
}
