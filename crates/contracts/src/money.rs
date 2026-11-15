//! Canonical exact RUB money shared across feature boundaries.
#![forbid(unsafe_code)]

pub const RUB_MICROS_PER_MINOR: i128 = 10_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MoneyError {
    NegativePrice,
    Overflow,
    FractionalMinor,
    InvalidId,
    InvalidWalletOwner,
    InvalidIdempotencyKey,
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
