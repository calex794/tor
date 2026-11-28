use crate::domain::{dispatch::DeferReason, quota::QuotaDecision, MailOutcome};

pub(super) enum Denial {
    Defer(DeferReason),
    Finish(MailOutcome),
    StopAndDefer,
    StopAndFinish(MailOutcome),
    LeaseLost,
}

pub(super) fn denial(decision: QuotaDecision) -> Option<Denial> {
    use Denial::*;
    use QuotaDecision::*;
    Some(match decision {
        Reserved => return None,
        GlobalPace => Defer(DeferReason::GlobalPace),
        RegistrationRecipientCooldown => Defer(DeferReason::RecipientCooldown),
        GlobalDailyLimit
        | TransactionalReserve
        | RegistrationRecipientDaily
        | RegistrationIpDaily => Defer(DeferReason::QuotaWindow),
        CostNewsPaused | CostMonthlyLimit => Defer(DeferReason::BudgetPaused),
        CostEmergencyStop | CostStateUnavailable | CostMonthBoundary => StopAndDefer,
        RecipientSuppressed(_) | RegistrationEvidenceRequired => Finish(MailOutcome::Cancelled),
        KeyMismatch => StopAndFinish(MailOutcome::Failed),
        AlreadyReserved => StopAndFinish(MailOutcome::Uncertain),
        StaleClaim => LeaseLost,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::suppression::SuppressionReason;

    #[test]
    fn only_a_new_reservation_can_continue_and_no_replay_can_defer() {
        assert!(denial(QuotaDecision::Reserved).is_none());
        assert!(matches!(
            denial(QuotaDecision::AlreadyReserved),
            Some(Denial::StopAndFinish(MailOutcome::Uncertain))
        ));
        assert!(matches!(
            denial(QuotaDecision::StaleClaim),
            Some(Denial::LeaseLost)
        ));
        assert!(matches!(
            denial(QuotaDecision::RecipientSuppressed(
                SuppressionReason::Complaint
            )),
            Some(Denial::Finish(MailOutcome::Cancelled))
        ));
        assert!(matches!(
            denial(QuotaDecision::CostEmergencyStop),
            Some(Denial::StopAndDefer)
        ));
        assert!(matches!(
            denial(QuotaDecision::GlobalPace),
            Some(Denial::Defer(DeferReason::GlobalPace))
        ));
    }
}
