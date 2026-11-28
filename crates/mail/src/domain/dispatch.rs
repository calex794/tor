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
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::GlobalPace => "global_pace",
            Self::RecipientCooldown => "recipient_cooldown",
            Self::DnsUnavailable => "dns_unavailable",
            Self::TransportUnavailable => "transport_unavailable",
            Self::ActivationUnavailable => "activation_unavailable",
        }
    }

    pub(crate) const fn seconds(self, generation: i32) -> i64 {
        let exponent = match generation {
            0 | 1 => 0,
            2 => 1,
            _ => 2,
        };
        match self {
            Self::GlobalPace => 5,
            Self::RecipientCooldown => 120,
            Self::DnsUnavailable | Self::TransportUnavailable | Self::ActivationUnavailable => {
                30 * (1 << exponent)
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_charge_backoff_is_bounded_and_preserves_cooldown() {
        assert_eq!(DeferReason::DnsUnavailable.seconds(1), 30);
        assert_eq!(DeferReason::TransportUnavailable.seconds(2), 60);
        assert_eq!(DeferReason::ActivationUnavailable.seconds(3), 120);
        assert_eq!(DeferReason::DnsUnavailable.seconds(i32::MAX), 120);
        assert_eq!(DeferReason::GlobalPace.seconds(1), 5);
        assert_eq!(DeferReason::RecipientCooldown.seconds(1), 120);
        assert_eq!(MAX_CLAIMS, 4);
    }
}
