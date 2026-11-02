//! D-06: fail-closed refund intent and *non-posting* Layer preflight.
//! No user refund route, policy catalog, financial reversal or approval exists.
//! All future approvals must recheck policy and first exposure inside the
//! same SERIALIZABLE PostgreSQL transaction that reverses the purchase.
use crate::domain::money::IdempotencyKey;
use crate::repositories::refund_preflight::PgLayerRefundPreflight;
use sha2::{Digest, Sha256};
use tor_contracts::paid_delivery::PositiveId;

pub const MAX_REFUND_REASON_CHARS: usize = 512;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefundGateError {
    /// No owned, internally evidenced paid term. Hide other actors' purchases.
    NotFound,
    InvalidReason,
    /// This checkout has no immutable refund policy/deadline from purchase.
    PolicyUnavailable,
    /// Access was revoked, but that does NOT prove a financial reversal.
    RevocationPresent,
    Unavailable,
}

/// A typed request intention, NOT a refund request fact or a permission to
/// return money. The 43-character key is already validated by IdempotencyKey.
#[derive(Clone, Debug)]
pub struct LayerRefundIntent {
    actor: PositiveId,
    term: PositiveId,
    reason: String,
    key: IdempotencyKey,
    canonical_request_sha256: [u8; 32],
}

impl LayerRefundIntent {
    pub fn new(
        actor: PositiveId,
        term: PositiveId,
        reason: &str,
        key: IdempotencyKey,
    ) -> Result<Self, RefundGateError> {
        let trimmed = reason.trim();
        if trimmed.is_empty()
            || trimmed.chars().count() > MAX_REFUND_REASON_CHARS
            || trimmed.chars().any(|c| c.is_control())
        {
            return Err(RefundGateError::InvalidReason);
        }
        let mut digest = Sha256::new();
        digest.update(b"tor.layer.refund.intent.v1");
        digest.update(actor.get().to_be_bytes());
        digest.update(term.get().to_be_bytes());
        digest.update((trimmed.len() as u64).to_be_bytes());
        digest.update(trimmed.as_bytes());
        Ok(Self {
            actor,
            term,
            reason: trimmed.to_owned(),
            key,
            canonical_request_sha256: digest.finalize().into(),
        })
    }

    pub fn actor(&self) -> PositiveId {
        self.actor
    }

    pub fn term(&self) -> PositiveId {
        self.term
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }

    pub fn key(&self) -> &IdempotencyKey {
        &self.key
    }

    pub fn canonical_digest(&self) -> [u8; 32] {
        self.canonical_request_sha256
    }
}

#[derive(Clone)]
pub struct LayerRefundService {
    repository: PgLayerRefundPreflight,
}

impl LayerRefundService {
    pub fn new(repository: PgLayerRefundPreflight) -> Self {
        Self { repository }
    }

    /// Currently always denies: no versioned refund policy/deadline was
    /// captured when the S031 closed-loop wallet purchase was committed.
    /// It never inserts a refund request or mutates entitlement/wallet state.
    pub async fn preflight(&self, intent: &LayerRefundIntent) -> Result<(), RefundGateError> {
        self.repository.inspect(intent).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> IdempotencyKey {
        IdempotencyKey::parse(&format!("{}8", "A".repeat(42))).unwrap()
    }

    #[test]
    fn d_06_canonical_intent_binds_actor_term_and_exact_reason() {
        let actor = PositiveId::new(3).unwrap();
        let term = PositiveId::new(7).unwrap();
        let first = LayerRefundIntent::new(actor, term, "  Не выдан доступ  ", key()).unwrap();
        let replay = LayerRefundIntent::new(actor, term, "Не выдан доступ", key()).unwrap();
        assert_eq!(first.reason(), "Не выдан доступ");
        assert_eq!(first.key().as_str(), key().as_str());
        assert_eq!(first.canonical_digest(), replay.canonical_digest());
        assert_ne!(
            first.canonical_digest(),
            LayerRefundIntent::new(actor, term, "Иная причина", key())
                .unwrap()
                .canonical_digest()
        );
        assert_ne!(
            first.canonical_digest(),
            LayerRefundIntent::new(actor, PositiveId::new(8).unwrap(), first.reason(), key())
                .unwrap()
                .canonical_digest()
        );
        assert_ne!(
            first.canonical_digest(),
            LayerRefundIntent::new(PositiveId::new(4).unwrap(), term, first.reason(), key())
                .unwrap()
                .canonical_digest()
        );
    }

    #[test]
    fn d_06_refund_reason_is_bounded_without_guessing_a_refund_policy() {
        let actor = PositiveId::new(3).unwrap();
        let term = PositiveId::new(7).unwrap();
        for reason in ["", " \t ", "bad\nreason", "bad\u{0000}reason"] {
            assert!(matches!(
                LayerRefundIntent::new(actor, term, reason, key()),
                Err(RefundGateError::InvalidReason)
            ));
        }
        assert!(LayerRefundIntent::new(actor, term, &"я".repeat(512), key()).is_ok());
        assert!(matches!(
            LayerRefundIntent::new(actor, term, &"я".repeat(513), key()),
            Err(RefundGateError::InvalidReason)
        ));
    }
}
