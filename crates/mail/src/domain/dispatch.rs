//! Common bounded queue scheduling. Only uncharged claims may be deferred.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeferReason {
    GlobalPace,
    RecipientCooldown,
    DnsUnavailable,
    TransportUnavailable,
    ActivationUnavailable,
}

impl DeferReason {
    pub(crate) const fn seconds(self) -> i64 {
        match self {
            Self::GlobalPace => 5,
            Self::RecipientCooldown => 120,
            Self::DnsUnavailable | Self::TransportUnavailable | Self::ActivationUnavailable => 30,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeferDecision {
    Deferred,
    Exhausted,
    Charged,
    Stale,
}

pub(crate) const MAX_CLAIMS: i32 = 4;
