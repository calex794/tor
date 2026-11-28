//! Trusted account/event adapters, never a browser request or arbitrary mail API.
use crate::domain::quota::{identity::QuotaKeySigner, QuotaKeys};
use std::future::Future;
use tor_contracts::mail::{MailIntent, MailPurpose, RegistrationMailEvidence};
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SourceFailure {
    Cancelled,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreflightFailure {
    InvalidRecipient,
    Unavailable,
}

/// Ephemeral message from the trusted source. No Debug or serialization.
/// Rendering must be deterministic for the same live event and address.
pub struct ResolvedMail {
    intent: MailIntent,
    pub(super) recipient: Zeroizing<String>,
    pub(super) subject: Zeroizing<String>,
    pub(super) body: Zeroizing<String>,
    registration: Option<RegistrationMailEvidence>,
}

impl ResolvedMail {
    pub fn new(
        intent: MailIntent,
        recipient: String,
        subject: String,
        body: String,
        registration: Option<RegistrationMailEvidence>,
    ) -> Self {
        Self {
            intent,
            recipient: Zeroizing::new(recipient),
            subject: Zeroizing::new(subject),
            body: Zeroizing::new(body),
            registration,
        }
    }

    pub(super) fn keys(&self, intent: MailIntent, signer: &QuotaKeySigner) -> Option<QuotaKeys> {
        if self.intent != intent {
            return None;
        }
        match (intent.purpose, self.registration) {
            (MailPurpose::RegistrationConfirmation, Some(evidence)) => signer
                .keys_for_registration(intent, self.recipient.as_str(), evidence)
                .ok(),
            (MailPurpose::RegistrationConfirmation, None) | (_, Some(_)) => None,
            (_, None) => signer.keys(self.recipient.as_str(), None).ok(),
        }
    }

    pub(super) fn same(&self, other: &Self) -> bool {
        self.intent == other.intent
            && self.recipient.as_str() == other.recipient.as_str()
            && self.subject.as_str() == other.subject.as_str()
            && self.body.as_str() == other.body.as_str()
            && self.registration == other.registration
    }
}

/// Implementations must resolve committed event ownership, live account,
/// current canonical delivery address and purpose-specific eligibility (news
/// subscription, registration evidence, etc.) on *every* resolve call.
/// No implementation may turn a timeout/unknown result into eligibility.
pub trait DispatchSource: Sync {
    fn resolve(
        &self,
        intent: MailIntent,
    ) -> impl Future<Output = Result<ResolvedMail, SourceFailure>> + Send;
}

/// One shared recipient-policy/DNS adapter, independent of event ownership.
/// A success must apply to the exact canonical transport mailbox checked;
/// a timeout or unknown result is never eligibility.
pub trait DispatchPreflight: Sync {
    fn check(&self, recipient: &str) -> impl Future<Output = Result<(), PreflightFailure>> + Send;
}
