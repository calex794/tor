//! Bounded successful-login security facts. Not RIL profiling or authentication.
use std::net::IpAddr;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoginEventKind {
    Registration,
    PasswordLogin,
    PasswordChange,
}

impl LoginEventKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Registration => "registration",
            Self::PasswordLogin => "password_login",
            Self::PasswordChange => "password_change",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UserAgentFamily {
    Edge,
    Chrome,
    Firefox,
    Safari,
    Other,
    Unknown,
}

impl UserAgentFamily {
    pub fn label(self) -> &'static str {
        match self {
            Self::Edge => "edge",
            Self::Chrome => "chrome",
            Self::Firefox => "firefox",
            Self::Safari => "safari",
            Self::Other => "other",
            Self::Unknown => "unknown",
        }
    }
}

/// No raw User-Agent, password, token, or request path is stored.
/// IP is None when the trusted-proxy boundary cannot establish a source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoginAuditContext {
    pub verified_ip: Option<IpAddr>,
    pub agent_family: UserAgentFamily,
    pub event_kind: LoginEventKind,
}
