//! Security-only observations of completed password login attempts.
//! No passwords, raw email, IP, token, or user-agent string may enter this type.
use super::access_audit::UserAgentFamily;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthAttemptOutcome {
    Success,
    BadPassword,
    UnknownAccount,
    RateLimited,
    InvalidForm,
    Unavailable,
    WorkerBusy,
}

impl AuthAttemptOutcome {
    pub fn label(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::BadPassword => "bad_password",
            Self::UnknownAccount => "unknown_account",
            Self::RateLimited => "rate_limited",
            Self::InvalidForm => "invalid_form",
            Self::Unavailable => "unavailable",
            Self::WorkerBusy => "worker_busy",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct AuthAttemptObservation {
    pub target_key: Option<[u8; 32]>,
    pub user_id: Option<i64>,
    pub source_key: Option<[u8; 32]>,
    pub subnet_key: Option<[u8; 32]>,
    pub agent: UserAgentFamily,
    pub outcome: AuthAttemptOutcome,
}
