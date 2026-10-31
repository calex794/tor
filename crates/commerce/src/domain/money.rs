//! D-01: RUB minor units, typed IDs and deterministic purchase intent.
//! These types do not grant paid access or mutate wallet balances.
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};

pub const RUB_MICROS_PER_MINOR: i128 = 10_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MoneyError {
    NegativePrice,
    Overflow,
    FractionalMinor,
    InvalidId,
    InvalidWalletOwner,
    InvalidIdempotencyKey,
    InvalidDiscountOffer,
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
        let minor =
            i64::try_from(micros / RUB_MICROS_PER_MINOR).map_err(|_| MoneyError::Overflow)?;
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
/// Its final sextet must have its two unused bits set to zero.
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
                b'A' | b'E'
                    | b'I'
                    | b'M'
                    | b'Q'
                    | b'U'
                    | b'Y'
                    | b'c'
                    | b'g'
                    | b'k'
                    | b'o'
                    | b's'
                    | b'w'
                    | b'0'
                    | b'4'
                    | b'8'
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PurchaseDiscountOffer {
    pub campaign_id: PositiveId,
    pub campaign_revision: i32,
    pub rule_version: i16,
    pub follow_period_id: PositiveId,
    pub discount_percent: i16,
    pub min_follow_days: i16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PurchaseCommand {
    pub actor_id: PositiveId,
    pub wallet_id: WalletId,
    pub edition: ChapterVersionRef,
    /// Final amount the reader agreed to pay, in kopecks.
    pub expected_price: MoneyMinor,
    /// Original list price, pinned separately from the final amount.
    pub expected_base_price: MoneyMinor,
    /// Exact campaign and continuous follow period shown in the quote.
    pub expected_discount_offer: Option<PurchaseDiscountOffer>,
    pub idempotency_key: IdempotencyKey,
    /// Derived from all typed quote values, never supplied as a hash by a client.
    pub canonical_request_sha256: [u8; 32],
}

impl PurchaseCommand {
    pub fn new(
        actor_id: PositiveId,
        wallet_id: WalletId,
        edition: ChapterVersionRef,
        expected_price: MoneyMinor,
        idempotency_key: IdempotencyKey,
    ) -> Result<Self, MoneyError> {
        if wallet_id.get() != actor_id.get() {
            return Err(MoneyError::InvalidWalletOwner);
        }
        if expected_price.get() == 0 {
            return Err(MoneyError::NegativePrice);
        }
        let mut command = Self {
            actor_id,
            wallet_id,
            edition,
            expected_price,
            expected_base_price: expected_price,
            expected_discount_offer: None,
            idempotency_key,
            canonical_request_sha256: [0; 32],
        };
        command.refresh_hash();
        Ok(command)
    }

    /// Bind a discounted purchase to the exact list price, campaign revision,
    /// and uninterrupted creator-follow period displayed to the reader.
    pub fn with_discount_offer(
        mut self,
        base_price: MoneyMinor,
        campaign_id: i64,
        campaign_revision: i32,
        rule_version: i16,
        follow_period_id: i64,
        discount_percent: i16,
        min_follow_days: i16,
    ) -> Result<Self, MoneyError> {
        if base_price.get() <= self.expected_price.get()
            || campaign_revision <= 0
            || rule_version != 1
            || !(1..=90).contains(&discount_percent)
            || !matches!(min_follow_days, 0 | 30 | 90 | 180)
        {
            return Err(MoneyError::InvalidDiscountOffer);
        }
        let campaign_id = PositiveId::new(campaign_id)?;
        let follow_period_id = PositiveId::new(follow_period_id)?;
        let discount_minor =
            i128::from(base_price.get()) * i128::from(discount_percent) / 100_i128;
        let paid_minor = i128::from(base_price.get()) - discount_minor;
        if discount_minor <= 0 || paid_minor <= 0 || paid_minor != i128::from(self.expected_price.get()) {
            return Err(MoneyError::InvalidDiscountOffer);
        }
        self.expected_base_price = base_price;
        self.expected_discount_offer = Some(PurchaseDiscountOffer {
            campaign_id,
            campaign_revision,
            rule_version,
            follow_period_id,
            discount_percent,
            min_follow_days,
        });
        self.refresh_hash();
        Ok(self)
    }

    fn refresh_hash(&mut self) {
        let mut hash = Sha256::new();
        hash.update(b"tor.chapter.purchase.v2");
        for value in [
            self.actor_id.get(),
            self.wallet_id.get(),
            self.edition.chapter_id.get(),
            self.edition.version_id.get(),
            self.expected_base_price.get(),
            self.expected_price.get(),
        ] {
            hash.update(value.to_be_bytes());
        }
        let offer = self.expected_discount_offer;
        for value in [
            offer.map_or(0, |v| v.campaign_id.get()),
            i64::from(offer.map_or(0, |v| v.campaign_revision)),
            i64::from(offer.map_or(0, |v| v.rule_version)),
            offer.map_or(0, |v| v.follow_period_id.get()),
            i64::from(offer.map_or(0, |v| v.discount_percent)),
            i64::from(offer.map_or(0, |v| v.min_follow_days)),
        ] {
            hash.update(value.to_be_bytes());
        }
        self.canonical_request_sha256 = hash.finalize().into();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PurchaseReceipt {
    pub purchase_id: PurchaseId,
    pub pinned: ChapterVersionRef,
    pub amount: MoneyMinor,
    pub balance_after: MoneyMinor,
    pub replayed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CheckoutError {
    InsufficientBalance,
    StaleOffer,
    Inaccessible,
    AlreadyOwned,
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
        assert_eq!(
            MoneyMinor::from_rub_micros_exact(-10_000),
            Err(MoneyError::NegativePrice)
        );
        assert_eq!(
            MoneyMinor::from_rub_micros_exact(10_001),
            Err(MoneyError::FractionalMinor)
        );
        assert_eq!(
            MoneyMinor::new(i64::MAX).unwrap().to_rub_micros(),
            i128::from(i64::MAX) * 10_000
        );
        assert_eq!(
            MoneyMinor::from_rub_micros_exact(i128::MAX),
            Err(MoneyError::FractionalMinor)
        );
        assert_eq!(
            MoneyMinor::from_rub_micros_exact(i128::MAX - (i128::MAX % 10_000)),
            Err(MoneyError::Overflow)
        );
    }

    #[test]
    fn d_01_checked_arithmetic_and_signed_ledger() {
        let high = MoneyMinor::new(i64::MAX).unwrap();
        let one = MoneyMinor::new(1).unwrap();
        assert_eq!(high.checked_add(one), Err(MoneyError::Overflow));
        assert_eq!(one.checked_sub(high), Err(MoneyError::NegativePrice));
        assert_eq!(high.checked_sub(one).unwrap().get(), i64::MAX - 1);
        assert_eq!(
            LedgerDelta::new(-1)
                .checked_add(LedgerDelta::new(1))
                .unwrap()
                .get(),
            0
        );
        assert_eq!(
            LedgerDelta::new(i64::MAX).checked_add(LedgerDelta::new(1)),
            Err(MoneyError::Overflow)
        );
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
        assert_eq!(
            IdempotencyKey::parse(&("A".repeat(42) + "B")),
            Err(MoneyError::InvalidIdempotencyKey)
        );
        assert_eq!(
            IdempotencyKey::parse(&("A".repeat(42) + "=")),
            Err(MoneyError::InvalidIdempotencyKey)
        );
        assert_eq!(
            IdempotencyKey::parse(&("A".repeat(42) + "/")),
            Err(MoneyError::InvalidIdempotencyKey)
        );
        assert_eq!(
            IdempotencyKey::parse(&"A".repeat(44)),
            Err(MoneyError::InvalidIdempotencyKey)
        );
    }

    fn request(price_minor: i64, wallet_id: i64) -> Result<PurchaseCommand, MoneyError> {
        let actor = PositiveId::new(1).unwrap();
        let wallet = WalletId::new(wallet_id).unwrap();
        let edition = ChapterVersionRef {
            chapter_id: PositiveId::new(3).unwrap(),
            version_id: PositiveId::new(4).unwrap(),
        };
        let key = IdempotencyKey::parse(&("A".repeat(42) + "8")).unwrap();
        PurchaseCommand::new(
            actor,
            wallet,
            edition,
            MoneyMinor::new(price_minor).unwrap(),
            key,
        )
    }

    #[test]
    fn d_01_command_binds_wallet_price_and_exact_version() {
        let command = request(100, 1).unwrap();
        assert_eq!(command.expected_price.get(), 100);
        assert_eq!(command.edition.version_id.get(), 4);
        assert_eq!(request(100, 2), Err(MoneyError::InvalidWalletOwner));
        assert_eq!(request(0, 1), Err(MoneyError::NegativePrice));
        assert_ne!(
            request(100, 1).unwrap().canonical_request_sha256,
            request(101, 1).unwrap().canonical_request_sha256
        );
        let plain = request(80, 1).unwrap();
        let discounted = request(80, 1)
            .unwrap()
            .with_discount_offer(
                MoneyMinor::new(100).unwrap(),
                10,
                1,
                20,
                20,
                90,
            )
            .unwrap();
        assert_ne!(
            plain.canonical_request_sha256,
            discounted.canonical_request_sha256,
            "the idempotency identity includes the original price and discount provenance"
        );
        assert_eq!(
            request(81, 1).unwrap().with_discount_offer(
                MoneyMinor::new(100).unwrap(),
                10,
                1,
                20,
                20,
                90,
            ),
            Err(MoneyError::InvalidDiscountOffer),
            "the final price must equal the exact integer discount calculation"
        );
    }
}
