//! HTTP-only mapping of a successful login into bounded security facts.
use super::login_limiter;
use axum::http::{header, HeaderMap};
use std::net::SocketAddr;
use tor_accounts::domain::access_audit::{
    LoginAuditContext, LoginEventKind, UserAgentFamily,
};

pub(super) fn context(
    headers: &HeaderMap,
    peer: SocketAddr,
    event_kind: LoginEventKind,
) -> LoginAuditContext {
    LoginAuditContext {
        verified_ip: login_limiter::verified_client_ip(peer.ip(), headers),
        agent_family: classify_user_agent(headers),
        event_kind,
    }
}

/// Persist only an allowlisted browser family. No arbitrary header values
/// enter account history or monitoring labels.
fn classify_user_agent(headers: &HeaderMap) -> UserAgentFamily {
    let Some(ua) = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
    else {
        return UserAgentFamily::Unknown;
    };
    if ua.len() > 512 || ua.chars().any(char::is_control) {
        return UserAgentFamily::Unknown;
    }
    if ua.contains("Edg/") || ua.contains("EdgiOS/") || ua.contains("Edge/") {
        UserAgentFamily::Edge
    } else if ua.contains("Firefox/") || ua.contains("FxiOS/") {
        UserAgentFamily::Firefox
    } else if ua.contains("Chrome/") || ua.contains("CriOS/") {
        UserAgentFamily::Chrome
    } else if ua.contains("Safari/") {
        UserAgentFamily::Safari
    } else {
        UserAgentFamily::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn audit_uses_direct_peer_not_forged_forwarded_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("203.0.113.42"));
        headers.insert("x-forwarded-for", HeaderValue::from_static("203.0.113.43"));
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_static("Mozilla/5.0 Chrome/120.0.0.0 Safari/537.36"),
        );
        let peer = SocketAddr::from(([198, 51, 100, 22], 12345));
        let facts = context(&headers, peer, LoginEventKind::PasswordLogin);
        assert_eq!(
            facts.verified_ip,
            Some(IpAddr::V4(Ipv4Addr::new(198, 51, 100, 22)))
        );
        assert_eq!(facts.agent_family, UserAgentFamily::Chrome);
    }

    #[test]
    fn invalid_or_long_user_agent_cannot_enter_audit() {
        let mut headers = HeaderMap::new();
        assert_eq!(classify_user_agent(&headers), UserAgentFamily::Unknown);
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_str(&"x".repeat(513)).unwrap(),
        );
        assert_eq!(classify_user_agent(&headers), UserAgentFamily::Unknown);
        headers.insert(header::USER_AGENT, HeaderValue::from_static("RandomBot/1"));
        assert_eq!(classify_user_agent(&headers), UserAgentFamily::Other);
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_static("Mozilla/5.0 Edg/120.0 Chrome/120.0 Safari/537.36"),
        );
        assert_eq!(classify_user_agent(&headers), UserAgentFamily::Edge);
    }
}
