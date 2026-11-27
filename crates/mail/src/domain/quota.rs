//! MAIL-01f1: conservative initial budget for the shared outbound provider.
pub mod identity;

pub const DAILY_LIMIT: i64 = 200;
pub const TRANSACTIONAL_RESERVE: i64 = 50;
pub const REGISTRATION_RECIPIENT_DAILY: i64 = 3;
pub const REGISTRATION_IP_DAILY: i64 = 10;

/// Opaque keyed digests, not raw mailbox or IP. Construct only through
/// the versioned signer; do not deserialize from untrusted inputs.
/// Unknown registration IPs share one restricted bucket; they never bypass it.
#[derive(Clone, Copy)]
pub struct QuotaKeys {
    recipient: [u8; 32],
    registration_ip: Option<[u8; 32]>,
    key_binding: [u8; 32],
    registration_intent: Option<tor_contracts::mail::MailIntent>,
}

impl QuotaKeys {
    pub fn recipient(&self) -> &[u8; 32] {
        &self.recipient
    }

    pub fn registration_ip(&self) -> Option<&[u8; 32]> {
        self.registration_ip.as_ref()
    }

    pub(crate) fn key_binding(&self) -> &[u8; 32] {
        &self.key_binding
    }

    pub fn signup_source(&self) -> tor_contracts::mail::SignupMailSource {
        tor_contracts::mail::SignupMailSource {
            recipient_key: self.recipient,
            registration_ip_key: self.registration_ip,
            key_binding: self.key_binding,
        }
    }

    pub(crate) fn matches_registration(&self, intent: tor_contracts::mail::MailIntent) -> bool {
        self.registration_intent == Some(intent)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaDecision {
    /// Only a newly committed reservation authorizes a single SMTP attempt.
    Reserved,
    AlreadyReserved,
    StaleClaim,
    KeyMismatch,
    RegistrationEvidenceRequired,
    RecipientSuppressed(super::suppression::SuppressionReason),
    CostStateUnavailable,
    CostEmergencyStop,
    CostNewsPaused,
    CostMonthlyLimit,
    CostMonthBoundary,
    GlobalDailyLimit,
    TransactionalReserve,
    GlobalPace,
    RegistrationRecipientDaily,
    RegistrationRecipientCooldown,
    RegistrationIpDaily,
}

impl QuotaDecision {
    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "reserved" => Some(Self::Reserved),
            "global_daily" => Some(Self::GlobalDailyLimit),
            "transactional_reserve" => Some(Self::TransactionalReserve),
            "global_pace" => Some(Self::GlobalPace),
            "recipient_daily" => Some(Self::RegistrationRecipientDaily),
            "recipient_cooldown" => Some(Self::RegistrationRecipientCooldown),
            "ip_daily" => Some(Self::RegistrationIpDaily),
            "stale_claim" => Some(Self::StaleClaim),
            _ => None,
        }
    }
}
