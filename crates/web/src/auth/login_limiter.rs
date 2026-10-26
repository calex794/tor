//! Distributed login and registration throttling, owned by account security.
use super::AuthState;
use axum::{extract::ConnectInfo, http::StatusCode};
use std::net::SocketAddr;
use tor_accounts::services::security::session_token_digest;

/// Bound Argon2 work by verified TCP peer IP, never forwarding headers.
pub(super) async fn limit(
    state: &AuthState,
    action: &str,
    email: &str,
    peer: ConnectInfo<SocketAddr>,
) -> Result<[u8; 32], StatusCode> {
    let identity_key = session_token_digest(&format!("auth:{action}:identity:{email}"));
    let ip = peer.0.ip().to_string();
    let address_key = session_token_digest(&format!("auth:{action}:peer:{ip}"));
    // No in-process counters: these limits apply across all server processes.
    let address_ok = state
        .accounts
        .permit_auth_attempt(&address_key, 60, 600)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if !address_ok {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    let identity_ok = state
        .accounts
        .permit_auth_attempt(&identity_key, 8, 600)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if !identity_ok {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    Ok(identity_key)
}
