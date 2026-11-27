//! MAIL-01g1: only verified, recipient-specific delivery failures suppress.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeliveryFeedback {
    RecipientDoesNotExist,
    SpamComplaint,
    TemporaryFailure,
    OtherPermanentFailure,
    Delivered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SuppressionReason {
    HardBounce,
    Complaint,
}

impl SuppressionReason {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HardBounce => "hard_bounce",
            Self::Complaint => "complaint",
        }
    }

    pub(crate) fn parse(value: &str) -> Option<Self> {
        match value {
            "hard_bounce" => Some(Self::HardBounce),
            "complaint" => Some(Self::Complaint),
            _ => None,
        }
    }
}

impl DeliveryFeedback {
    pub fn suppression_reason(self) -> Option<SuppressionReason> {
        match self {
            Self::RecipientDoesNotExist => Some(SuppressionReason::HardBounce),
            Self::SpamComplaint => Some(SuppressionReason::Complaint),
            _ => None,
        }
    }
}

/// Identifies an already reserved submission. Only the trusted SMTP worker
/// or a verified provider-event correlator may resolve this reference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttemptRef {
    pub outbox_id: i64,
    pub generation: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FeedbackDecision {
    Applied,
    Unchanged,
    Ignored,
    Uncorrelated,
    UnboundKey,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_recipient_specific_permanent_failure_and_complaint_suppress() {
        use DeliveryFeedback::*;
        assert_eq!(
            RecipientDoesNotExist.suppression_reason(),
            Some(SuppressionReason::HardBounce)
        );
        assert_eq!(
            SpamComplaint.suppression_reason(),
            Some(SuppressionReason::Complaint)
        );
        for feedback in [TemporaryFailure, OtherPermanentFailure, Delivered] {
            assert_eq!(feedback.suppression_reason(), None);
        }
    }
}
