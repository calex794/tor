use crate::domain::{dispatch::DeferReason, quota::QuotaDecision};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum QuotaAction {
    Proceed,
    Defer(DeferReason),
    Cancel,
    Stop,
    Quarantine,
}

/// Exhaustive classification; adding a new quota outcome requires a
/// reviewed scheduling decision. Only Reserved permits proof generation.
pub(super) fn quota_action(decision: QuotaDecision) -> QuotaAction {
    use QuotaAction::*;
    use QuotaDecision::*;
    match decision {
        Reserved => Proceed,
        GlobalPace => Defer(DeferReason::GlobalPace),
        PasswordResetRecipientCooldown | RegistrationRecipientCooldown => {
            Defer(DeferReason::RecipientCooldown)
        }
        RecipientSuppressed(_)
        | GlobalDailyLimit
        | RegistrationRecipientDaily
        | RegistrationIpDaily
        | PasswordResetRecipientDaily
        | PasswordResetIpDaily
        | TransactionalReserve => Cancel,
        CostStateUnavailable
        | CostEmergencyStop
        | CostNewsPaused
        | CostMonthlyLimit
        | CostMonthBoundary
        | DispatchNotAuthorized
        | KeyMismatch
        | RegistrationEvidenceRequired
        | PasswordResetEvidenceRequired => Stop,
        AlreadyReserved | StaleClaim | OperatorTestDaily | OperatorTestCooldown => Quarantine,
    }
}
