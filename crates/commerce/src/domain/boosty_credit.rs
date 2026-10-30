//! D-10/S041: evidence-bound wallet credit for one *paid* Boosty subscription charge.
//! This type is NOT provider authentication. A trusted, read-only collector must
//! prove each event ID, payer and net RUB amount before constructing it.
use async_trait::async_trait;
use sha2::{Digest, Sha256};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubscriptionChargeKind {
    FirstPayment,
    Renewal,
}

impl SubscriptionChargeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::FirstPayment => "subscribe",
            Self::Renewal => "recurrent",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoostyCreditError {
    InvalidEvidence,
    UnlinkedPayer,
    ConflictingEvidence,
    Overflow,
    Unavailable,
}

/// The ID must identify a specific charge, not a subscription, subscriber
/// snapshot, balance delta, date, or a hash of a changing API response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VerifiedSubscriptionCharge {
    operation_id: String,
    provider_user_id: i64,
    amount_minor: i64,
    kind: SubscriptionChargeKind,
}

impl VerifiedSubscriptionCharge {
    pub fn new(
        operation_id: &str,
        provider_user_id: i64,
        amount_minor: i64,
        kind: SubscriptionChargeKind,
    ) -> Result<Self, BoostyCreditError> {
        if operation_id.is_empty()
            || operation_id.len() > 120
            || !operation_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b':' | b'.'))
            || provider_user_id <= 0
            || amount_minor <= 0
        {
            return Err(BoostyCreditError::InvalidEvidence);
        }
        Ok(Self {
            operation_id: operation_id.to_owned(),
            provider_user_id,
            amount_minor,
            kind,
        })
    }

    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub fn provider_user_id(&self) -> i64 {
        self.provider_user_id
    }

    pub fn amount_minor(&self) -> i64 {
        self.amount_minor
    }

    pub fn kind(&self) -> SubscriptionChargeKind {
        self.kind
    }

    /// Reject a replay of the same external ID with different monetary facts.
    pub fn fingerprint(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(b"tor.boosty.charge.v1");
        hash.update((self.operation_id.len() as u64).to_be_bytes());
        hash.update(self.operation_id.as_bytes());
        hash.update(self.provider_user_id.to_be_bytes());
        hash.update(self.amount_minor.to_be_bytes());
        hash.update(self.kind.as_str().as_bytes());
        hash.finalize().into()
    }

    /// An immutable, bounded wallet journal reference derived only from the
    /// provider-issued charge identity, never from user text or a payment date.
    pub fn journal_reference(&self) -> String {
        let digest = Sha256::digest(self.operation_id.as_bytes());
        let mut reference = String::from("boosty:charge:");
        for byte in digest {
            reference.push(char::from_digit(u32::from(byte >> 4), 16).unwrap());
            reference.push(char::from_digit(u32::from(byte & 0x0f), 16).unwrap());
        }
        reference
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BoostyCreditReceipt {
    pub user_id: i64,
    pub amount_minor: i64,
    /// Historical snapshot at the original credit (not the current balance).
    pub balance_after_minor: i64,
    pub replayed: bool,
}

#[async_trait]
pub trait BoostyCreditRepository: Send + Sync {
    async fn credit_once(
        &self,
        charge: &VerifiedSubscriptionCharge,
    ) -> Result<BoostyCreditReceipt, BoostyCreditError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d_10_requires_one_real_charge_identity_and_positive_net_money() {
        let kind = SubscriptionChargeKind::Renewal;
        for bad in ["", " ", "my subscription!", &"x".repeat(121)] {
            assert_eq!(
                VerifiedSubscriptionCharge::new(bad, 1, 100, kind),
                Err(BoostyCreditError::InvalidEvidence)
            );
        }
        assert_eq!(
            VerifiedSubscriptionCharge::new("charge-1", 0, 100, kind),
            Err(BoostyCreditError::InvalidEvidence)
        );
        assert_eq!(
            VerifiedSubscriptionCharge::new("charge-1", 1, 0, kind),
            Err(BoostyCreditError::InvalidEvidence)
        );
    }

    #[test]
    fn d_10_fingerprint_detects_changed_amount_payer_or_charge_type() {
        let a = VerifiedSubscriptionCharge::new(
            "external-123",
            71,
            8000,
            SubscriptionChargeKind::FirstPayment,
        )
        .unwrap();
        let changed_amount = VerifiedSubscriptionCharge::new(
            "external-123",
            71,
            8001,
            SubscriptionChargeKind::FirstPayment,
        )
        .unwrap();
        let changed_kind = VerifiedSubscriptionCharge::new(
            "external-123",
            71,
            8000,
            SubscriptionChargeKind::Renewal,
        )
        .unwrap();
        assert_ne!(a.fingerprint(), changed_amount.fingerprint());
        assert_ne!(a.fingerprint(), changed_kind.fingerprint());
        assert_eq!(a.journal_reference(), changed_amount.journal_reference());
        assert!(a.journal_reference().len() <= 160);
    }
}
