//! D-01: RUB minor units, typed IDs and deterministic purchase intent.
//! These types do not grant paid access or mutate wallet balances.
use async_trait::async_trait;
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};

pub const RUB_MICROS_PER_MINOR: i128 = 10_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MoneyError {
    NegativePrice,
    Overflow,
    FractionalMinor,
    InvalidId,
    InvalidRevision,
    InvalidIdempotencyKey,
    InvalidRequestHash,
}

/// Nonnegative integer RUB kopecks. Never accepts floats.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MoneyMinor(i64);

impl MoneyMinor {
    pub fn new(value: i64) -> Result<Self, MoneyError> {
        if value < 0 {
            return Err(MoneyError::NegativePrice);
        }
        Ok(Self(value))
    }

    pub fn get(self) -> i64 {
        self.0
    }

    pub fn checked_add(self, other: Self) -> Result<Self, MoneyError> {
        self.0
            .checked_add(other.0)
            .ok_or(MoneyError::Overflow)
            .and_then(Self::new)
    }

    pub fn checked_sub(self, other: Self) -> Result<Self, MoneyError> {
        self.0
            .checked_sub(other.0)
            .ok_or(MoneyError::Overflow)
            .and_then(Self::new)
    }

    /// Conversion from legacy ad micros is exact or rejected, never rounded.
    pub fn from_rub_micros_exact(micros: i128) -> Result<Self, MoneyError> {
        if micros % RUB_MICROS_PER_MINOR != 0 {
            return Err(MoneyError::FractionalMinor);
        }
        let minor = i64::try_from(micros / RUB_MICROS_PER_MINOR)
            .map_err(|_| MoneyError::Overflow)?;
        Self::new(minor)
    }

    pub fn to_rub_micros(self) -> i128 {
        i128::from(self.0) * RUB_MICROS_PER_MINOR
    }
}

/// Only immutable ledger movements may be negative.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LedgerDelta(i64);

impl LedgerDelta {
    pub fn new(value: i64) -> Self {
        Self(value)
    }

    pub fn get(self) -> i64 {
        self.0
    }

    pub fn checked_add(self, other: Self) -> Result<Self, MoneyError> {
        self.0
            .checked_add(other.0)
            .map(Self)
            .ok_or(MoneyError::Overflow)
    }
}

macro_rules! positive_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        pub struct $name(i64);
        impl $name {
            pub fn new(value: i64) -> Result<Self, MoneyError> {
                if value <= 0 {
                    return Err(MoneyError::InvalidId);
                }
                Ok(Self(value))
            }

            pub fn get(self) -> i64 {
                self.0
            }
        }
    };
}

positive_id!(WalletId);
positive_id!(PurchaseId);
positive_id!(EntitlementId);

/// A canonical 32-byte random key encoded as 43 unpadded base64url chars.
/// Its last base64 sextet must have zero unused bits (four possible bits used).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct IdempotencyKey(String);

impl IdempotencyKey {
    pub fn parse(value: &str) -> Result<Self, MoneyError> {
        let bytes = value.as_bytes();
        if bytes.len() != 43
            || !bytes[..42]
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || *b == b'-' || *b == b'_')
            || !matches!(
                bytes[42],
                b'A' | b'E' | b'I' | b'M' | b'Q' | b'U' | b'Y' |
                b'c' | b'g' | b'k' | b'o' | b's' | b'w' |
                b'0' | b'4' | b'8'
            )
        {
            return Err(MoneyError::InvalidIdempotencyKey);
        }
        Ok(Self(value.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PurchaseCommand {
    pub actor_id: PositiveId,
    pub wallet_id: WalletId,
    pub edition: ChapterVersionRef,
    pub expected_price: MoneyMinor,
    pub expected_offer_revision: i64,
    pub idempotency_key: IdempotencyKey,
    /// SHA-256 of the canonical request, not of a raw JSON serialization.
    pub canonical_request_sha256: [u8; 32],
}

impl PurchaseCommand {
    pub fn new(
        actor_id: PositiveId,
        wallet_id: WalletId,
        edition: ChapterVersionRef,
        expected_price: MoneyMinor,
        expected_offer_revision: i64,
        idempotency_key: IdempotencyKey,
        canonical_request_sha256: [u8; 32],
    ) -> Result<Self, MoneyError> {
        if expected_offer_revision <= 0 {
            return Err(MoneyError::InvalidRevision);
        }
        if canonical_request_sha256 == [0; 32] {
            return Err(MoneyError::InvalidRequestHash);
        }
        Ok(Self {
            actor_id,
            wallet_id,
            edition,
            expected_price,
            expected_offer_revision,
            idempotency_key,
            canonical_request_sha256,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PurchaseReceipt {
    pub purchase_id: PurchaseId,
    pub entitlement_id: EntitlementId,
    pub pinned: ChapterVersionRef,
    pub amount: MoneyMinor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckoutError {
    InsufficientBalance,
    StaleOffer,
    Inaccessible,
    DuplicateKeyDifferentRequest,
    Unavailable,
}

/// D alone owns the atomic PostgreSQL wallet + ledger + pinned edition commit.
/// Reusing the same actor, key and canonical hash must return the same receipt.
#[async_trait]
pub trait CheckoutRepository: Send + Sync {
    async fn checkout_once(
        &self,
        command: &PurchaseCommand,
    ) -> Result<PurchaseReceipt, CheckoutError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn d_01_exact_minor_values_never_round() {
        assert_eq!(MoneyMinor::new(-1), Err(MoneyError::NegativePrice));
        assert_eq!(MoneyMinor::new(0).unwrap().get(), 0);
        assert_eq!(MoneyMinor::from_rub_micros_exact(10_000).unwrap().get(), 1);
        assert_eq!(MoneyMinor::from_rub_micros_exact(-10_000), Err(MoneyError::NegativePrice));
        assert_eq!(MoneyMinor::from_rub_micros_exact(10_001), Err(MoneyError::FractionalMinor));
        assert_eq!(MoneyMinor::new(i64::MAX).unwrap().to_rub_micros(),
            i128::from(i64::MAX) * 10_000);
        assert_eq!(MoneyMinor::from_rub_micros_exact(i128::MAX),
            Err(MoneyError::FractionalMinor));
        assert_eq!(MoneyMinor::from_rub_micros_exact(i128::MAX - (i128::MAX % 10_000)),
            Err(MoneyError::Overflow));
    }

    #[test]
    fn d_01_checked_arithmetic_and_signed_ledger() {
        let high = MoneyMinor::new(i64::MAX).unwrap();
        let one = MoneyMinor::new(1).unwrap();
        assert_eq!(high.checked_add(one), Err(MoneyError::Overflow));
        assert_eq!(one.checked_sub(high), Err(MoneyError::NegativePrice));
        assert_eq!(high.checked_sub(one).unwrap().get(), i64::MAX - 1);
        assert_eq!(LedgerDelta::new(-1).checked_add(LedgerDelta::new(1)).unwrap().get(), 0);
        assert_eq!(LedgerDelta::new(i64::MAX).checked_add(LedgerDelta::new(1)),
            Err(MoneyError::Overflow));
    }

    #[test]
    fn d_01_all_ids_are_positive() {
        assert_eq!(WalletId::new(0), Err(MoneyError::InvalidId));
        assert_eq!(PurchaseId::new(-1), Err(MoneyError::InvalidId));
        assert_eq!(EntitlementId::new(0), Err(MoneyError::InvalidId));
        assert_eq!(WalletId::new(7).unwrap().get(), 7);
    }

    #[test]
    fn d_01_unpadded_base64url_key_has_canonical_final_bits() {
        let valid = format!("{}8", "A".repeat(42));
        assert_eq!(IdempotencyKey::parse(&valid).unwrap().as_str(), valid);
        assert_eq!(IdempotencyKey::parse(&("A".repeat(42) + "B")),
            Err(MoneyError::InvalidIdempotencyKey));
        assert_eq!(IdempotencyKey::parse(&("A".repeat(42) + "=")),
            Err(MoneyError::InvalidIdempotencyKey));
        assert_eq!(IdempotencyKey::parse(&("A".repeat(42) + "/")),
            Err(MoneyError::InvalidIdempotencyKey));
        assert_eq!(IdempotencyKey::parse(&"A".repeat(44)),
            Err(MoneyError::InvalidIdempotencyKey));
    }

    #[test]
    fn d_01_command_requires_locked_price_revision_and_request_identity() {
        let actor = PositiveId::new(1).unwrap();
        let wallet = WalletId::new(2).unwrap();
        let edition = ChapterVersionRef {
            chapter_id: PositiveId::new(3).unwrap(),
            version_id: PositiveId::new(4).unwrap(),
        };
        let key = IdempotencyKey::parse(&("A".repeat(42) + "8")).unwrap();
        let price = MoneyMinor::new(1).unwrap();
        let command = PurchaseCommand::new(actor, wallet, edition, price, 1, key.clone(), [9; 32])
            .unwrap();
        assert_eq!(command.expected_price.get(), 1);
        assert_eq!(command.edition.version_id.get(), 4);
        assert_eq!(PurchaseCommand::new(actor, wallet, edition, price, 0, key.clone(), [9; 32]),
            Err(MoneyError::InvalidRevision));
        assert_eq!(PurchaseCommand::new(actor, wallet, edition, price, 1, key, [0; 32]),
            Err(MoneyError::InvalidRequestHash));
    }
}
