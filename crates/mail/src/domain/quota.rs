//! MAIL-01f1: conservative initial budget for the shared outbound provider.
pub const DAILY_LIMIT: i64 = 200;
pub const TRANSACTIONAL_RESERVE: i64 = 50;
pub const REGISTRATION_RECIPIENT_DAILY: i64 = 3;
pub const REGISTRATION_IP_DAILY: i64 = 10;

/// Opaque keyed digests, not raw mailbox or IP. The future trusted caller
/// derives these using stable, separate HMAC namespaces and a recovery key.
/// Unknown registration IPs share one restricted bucket; they never bypass it.
#[derive(Clone, Copy)]
pub struct QuotaKeys {
    pub recipient: [u8; 32],
    pub registration_ip: Option<[u8; 32]>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QuotaDecision {
    /// Only a newly committed reservation authorizes a single SMTP attempt.
    Reserved,
    AlreadyReserved,
    StaleClaim,
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
