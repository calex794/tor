//! Reference-only dispatch scheduling. No SMTP content or provider error text.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeferReason {
    SourceUnavailable,
    DnsUnavailable,
    ProviderUnavailable,
    GlobalPace,
    RecipientCooldown,
    QuotaWindow,
    BudgetPaused,
}

impl DeferReason {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::SourceUnavailable => "source_unavailable",
            Self::DnsUnavailable => "dns_unavailable",
            Self::ProviderUnavailable => "provider_unavailable",
            Self::GlobalPace => "global_pace",
            Self::RecipientCooldown => "recipient_cooldown",
            Self::QuotaWindow => "quota_window",
            Self::BudgetPaused => "budget_paused",
        }
    }

    pub(crate) const fn delay_seconds(self, generation: i32) -> i64 {
        let exponent = match generation {
            0 | 1 => 0,
            2 => 1,
            3 => 2,
            4 => 3,
            _ => 4,
        };
        match self {
            Self::SourceUnavailable | Self::DnsUnavailable | Self::ProviderUnavailable => {
                30 * (1 << exponent)
            }
            Self::GlobalPace => 2,
            Self::RecipientCooldown => 120,
            Self::QuotaWindow => 86400,
            Self::BudgetPaused => 3600,
        }
    }
}

pub const MAX_PRE_SUBMISSION_CLAIMS: i32 = 6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeferOutcome {
    Deferred,
    Exhausted,
    Refused,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_schedule_never_spins_on_dns_and_respects_quota_windows() {
        assert_eq!(DeferReason::DnsUnavailable.delay_seconds(1), 30);
        assert_eq!(DeferReason::DnsUnavailable.delay_seconds(5), 480);
        assert_eq!(DeferReason::DnsUnavailable.delay_seconds(i32::MAX), 480);
        assert_eq!(DeferReason::GlobalPace.delay_seconds(1), 2);
        assert_eq!(DeferReason::RecipientCooldown.delay_seconds(1), 120);
        assert_eq!(DeferReason::QuotaWindow.delay_seconds(1), 86400);
    }
}
