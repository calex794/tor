//! Capability cookie for S050. A fresh random 256-bit token is never stored raw.
use super::{session_user, AuthState};
use crate::auth_cookies::{cookie_value, valid_token};
use axum::http::{HeaderMap, StatusCode};
use tor_accounts::services::security::{generate_session_token, session_token_digest};
use tor_support::domain::Requester;

pub(super) fn cookie_name(state: &AuthState) -> &'static str {
    if state.cookies.secure {
        "__Host-tor-support"
    } else {
        "tor-dev-support"
    }
}

fn cookie(state: &AuthState, token: &str, age: u32) -> String {
    let secure = if state.cookies.secure { "; Secure" } else { "" };
    format!(
        "{}={token}; Path=/; Max-Age={age}; HttpOnly; SameSite=Strict{secure}",
        cookie_name(state)
    )
}

pub(super) async fn resolve(
    state: &AuthState,
    headers: &HeaderMap,
) -> Result<(Requester, Vec<String>), StatusCode> {
    let user = session_user(state, headers).await?;
    let token = cookie_value(headers, cookie_name(state)).filter(|t| valid_token(t));
    match (user, token) {
        (Some(id), Some(token)) => Ok((
            Requester::User {
                id,
                guest: Some(session_token_digest(&format!("tor-support:{token}"))),
            },
            vec![cookie(state, "", 0)],
        )),
        (Some(id), None) => Ok((Requester::User { id, guest: None }, Vec::new())),
        (None, Some(token)) => Ok((
            Requester::Guest(session_token_digest(&format!("tor-support:{token}"))),
            Vec::new(),
        )),
        (None, None) => {
            let token = generate_session_token();
            Ok((
                Requester::Guest(session_token_digest(&format!("tor-support:{token}"))),
                vec![cookie(state, &token, 30 * 86400)],
            ))
        }
    }
}

/// NEwBOO support_message canonicalizes to the default action scope:
/// 120 authenticated / 60 anonymous attempts per minute. Distributed PG state,
/// verified proxy address only; an unknown loopback never becomes a global key.
pub(super) async fn limit(
    state: &AuthState,
    who: &Requester,
    headers: &HeaderMap,
    peer: Option<axum::extract::ConnectInfo<std::net::SocketAddr>>,
) -> Result<(), StatusCode> {
    let (identity, attempts) = match who {
        Requester::User { id, .. } => (format!("support:default:user:{id}"), 120),
        Requester::Guest(hash) => {
            let ip = peer.and_then(|p| super::login_limiter::verified_client_ip(p.0.ip(), headers));
            let identity = if let Some(ip) = ip {
                format!("support:default:ip:{ip}")
            } else {
                // Fallback for in-process tests and unverified local transport.
                let encoded: String = hash.iter().map(|b| format!("{b:02x}")).collect();
                format!("support:default:guest:{encoded}")
            };
            (identity, 60)
        }
    };
    let key = session_token_digest(&identity);
    let allowed = state
        .accounts
        .permit_auth_attempt(&key, attempts, 60)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if !allowed {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    Ok(())
}
