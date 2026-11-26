pub mod quota;

use tor_contracts::mail::MailIntent;

/// A claim generation fences late results from a previous claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MailClaim {
    pub id: i64,
    pub generation: i32,
    pub intent: MailIntent,
}

/// SMTP acceptance is not proof of delivery to the recipient's inbox.
/// An ambiguous result must not be automatically submitted again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MailOutcome {
    Accepted,
    Failed,
    Uncertain,
    Cancelled,
}

impl MailOutcome {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Accepted => "accepted",
            Self::Failed => "failed",
            Self::Uncertain => "uncertain",
            Self::Cancelled => "cancelled",
        }
    }
}
