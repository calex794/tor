//! Distributed login and registration throttling, owned by account security.
use super::AuthState;
use axum::{
    extract::ConnectInfo,
    http::{HeaderMap, StatusCode},
};
use std::net::{IpAddr, SocketAddr};
use tor_accounts::services::security::session_token_digest;
use tor_accounts::services::auth_observation::AuthObservationSigner;

/// Only use X-Real-IP from a verified loopback reverse proxy when the
/// operator has configured Nginx to overwrite it with $remote_addr.
/// Never trust client-provided X-Forwarded-For, and never treat Nginx's
/// loopback socket address as the address shared by every visitor.
fn rate_limit_ip(peer: IpAddr, headers: &HeaderMap, trust_nginx: bool) -> Option<IpAddr> {
    if !peer.is_loopback() {
        return Some(peer);
    }
    if !trust_nginx {
        return None;
    }
    let mut values = headers.get_all("x-real-ip").iter();
    let value: IpAddr = values.next()?.to_str().ok()?.parse().ok()?;
    if values.next().is_some() || value.is_loopback() || value.is_unspecified() {
        return None;
    }
    Some(value)
}

pub(super) fn verified_client_ip(peer: IpAddr, headers: &HeaderMap) -> Option<IpAddr> {
    let trust_nginx = std::env::var("TOR_AUTH_TRUST_NGINX_REAL_IP").as_deref() == Ok("1");
    rate_limit_ip(peer, headers, trust_nginx)
}

/// Bound Argon2 work per account, and per verified client IP when available.
/// The old implementation keyed all Nginx visitors to 127.0.0.1 and could
/// lock everyone out after 60 unrelated login attempts in ten minutes.
pub(super) async fn limit(
    state: &AuthState,
    action: &str,
    email: &str,
    headers: &HeaderMap,
    peer: ConnectInfo<SocketAddr>,
) -> Result<[u8; 32], StatusCode> {
    let identity_key = session_token_digest(&format!("auth:{action}:identity:{email}"));
    if let Some(ip) = verified_client_ip(peer.0.ip(), headers) {
        let address_key = session_token_digest(&format!("auth:{action}:peer:{ip}"));
        let address_ok = state
            .accounts
            .permit_auth_attempt(&address_key, 60, 600)
            .await
            .map_err(|_| {
                eprintln!("TOR_AUTH_UNAVAILABLE stage=ip_rate_limit");
                StatusCode::SERVICE_UNAVAILABLE
            })?;
        if !address_ok {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
    }
    // Never fall back to a global loopback IP counter. Account-specific
    // throttling and the bounded Argon2 worker still apply in this case.
    let identity_ok = state
        .accounts
        .permit_auth_attempt(&identity_key, 8, 600)
        .await
        .map_err(|_| {
            eprintln!("TOR_AUTH_UNAVAILABLE stage=identity_rate_limit");
            StatusCode::SERVICE_UNAVAILABLE
        })?;
    if !identity_ok {
        return Err(StatusCode::TOO_MANY_REQUESTS);
    }
    Ok(identity_key)
}

/// AB-02 mode is explicit: no unreviewed spray lock in production.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SprayMode {
    Off,
    Observe,
    Enforce,
}

fn parse_spray_mode(value: &str) -> Option<SprayMode> {
    match value {
        "off" => Some(SprayMode::Off),
        "observe" => Some(SprayMode::Observe),
        "enforce" => Some(SprayMode::Enforce),
        _ => None,
    }
}

#[derive(Clone, Copy)]
pub(super) struct SprayGuard {
    ip: [u8; 32],
    subnet: [u8; 32],
    target: [u8; 32],
    mode: SprayMode,
}

impl SprayGuard {
    pub(super) fn enforcing(&self) -> bool {
        self.mode == SprayMode::Enforce
    }
}

pub(super) enum SprayError {
    Locked(u32),
    Unavailable,
}

/// Only an operator-enabled, independently keyed HMAC detector can change
/// login behavior. A missing key in enforce mode is an explicit 503, not an
/// accidental unlimited bypass. Unknown proxy peers never share a bucket.
pub(super) async fn spray_start(
    state: &AuthState,
    normalized_email: &str,
    verified_ip: Option<IpAddr>,
) -> Result<Option<SprayGuard>, SprayError> {
    let mode = std::env::var("TOR_AUTH_SPRAY_MODE").unwrap_or_else(|_| "off".to_owned());
    let mode = parse_spray_mode(&mode).ok_or(SprayError::Unavailable)?;
    if mode == SprayMode::Off {
        return Ok(None);
    }
    let Some(ip) = verified_ip else {
        return Ok(None);
    };
    let signer = std::env::var("TOR_AUTH_SPRAY_KEY")
        .ok()
        .and_then(|key| AuthObservationSigner::from_hex(&key));
    let Some(signer) = signer else {
        return if mode == SprayMode::Observe {
            Ok(None)
        } else {
            Err(SprayError::Unavailable)
        };
    };
    let (source, subnet) = signer.source(ip);
    if mode == SprayMode::Enforce {
        let retry = state
            .accounts
            .spray_retry_after(&source)
            .await
            .map_err(|_| SprayError::Unavailable)?;
        if let Some(seconds) = retry {
            return Err(SprayError::Locked(seconds));
        }
    }
    Ok(Some(SprayGuard {
        ip: source,
        subnet,
        target: signer.target(normalized_email),
        mode,
    }))
}

/// The best-effort observation queue is not the source of truth. When enforce
/// mode is enabled, failure to persist the evidence is surfaced to the caller.
pub(super) async fn spray_failure(state: &AuthState, guard: &SprayGuard) -> bool {
    state
        .accounts
        .record_spray_failure(
            &guard.ip,
            &guard.subnet,
            &guard.target,
            guard.enforcing(),
        )
        .await
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn spray_modes_require_explicit_selection() {
        assert_eq!(parse_spray_mode("off"), Some(SprayMode::Off));
        assert_eq!(parse_spray_mode("observe"), Some(SprayMode::Observe));
        assert_eq!(parse_spray_mode("enforce"), Some(SprayMode::Enforce));
        assert_eq!(parse_spray_mode("enabled"), None);
    }

    #[test]
    fn loopback_reverse_proxy_does_not_share_a_global_limit() {
        let headers = HeaderMap::new();
        assert_eq!(
            rate_limit_ip(Ipv4Addr::LOCALHOST.into(), &headers, false),
            None
        );
        assert_eq!(
            rate_limit_ip(Ipv6Addr::LOCALHOST.into(), &headers, false),
            None
        );
    }

    #[test]
    fn unsolicited_forwarding_headers_never_override_a_direct_peer() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("198.51.100.41"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("198.51.100.42"));
        let remote = IpAddr::V4(Ipv4Addr::new(192, 0, 2, 10));
        assert_eq!(rate_limit_ip(remote, &headers, true), Some(remote));
        assert_eq!(
            rate_limit_ip(Ipv4Addr::LOCALHOST.into(), &headers, false),
            None
        );
    }

    #[test]
    fn explicitly_configured_loopback_proxy_accepts_one_canonical_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("198.51.100.41"));
        assert_eq!(
            rate_limit_ip(Ipv4Addr::LOCALHOST.into(), &headers, true),
            Some(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 41)))
        );
        headers.append("x-real-ip", HeaderValue::from_static("198.51.100.99"));
        assert_eq!(
            rate_limit_ip(Ipv4Addr::LOCALHOST.into(), &headers, true),
            None
        );
    }

    #[test]
    fn invalid_or_loopback_proxy_header_cannot_be_used_as_a_shared_key() {
        let mut headers = HeaderMap::new();
        for raw in [
            "127.0.0.1",
            "0.0.0.0",
            "not-an-ip",
            "198.51.100.41, 127.0.0.1",
        ] {
            headers.insert("x-real-ip", HeaderValue::from_str(raw).unwrap());
            assert_eq!(
                rate_limit_ip(Ipv4Addr::LOCALHOST.into(), &headers, true),
                None
            );
        }
    }
}
