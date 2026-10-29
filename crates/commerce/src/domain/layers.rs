//! D-05: immutable offer identity and discrete paid layer terms.
use crate::domain::money::{IdempotencyKey, MoneyMinor, WalletId};
use tor_contracts::paid_delivery::PositiveId;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerTermKind {
    Prepaid30Days,
    Lifetime,
}
impl LayerTermKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepaid30Days => "prepaid_30d",
            Self::Lifetime => "lifetime",
        }
    }
}
#[derive(Clone, Debug)]
pub struct LayerCheckoutCommand {
    pub actor: PositiveId,
    pub wallet: WalletId,
    pub layer: PositiveId,
    pub offer: PositiveId,
    pub version: PositiveId,
    pub expected_price: MoneyMinor,
    pub key: IdempotencyKey,
}
/// A Layer owned by the authenticated author; never a purchase grant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedLayerSummary {
    pub layer_id: i64,
    pub title: String,
}

/// A read-only Layer picker entry. UI eligibility is advisory:
/// add_book() always revalidates membership under the canonical Book lock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerBookCandidate {
    pub book_id: i64,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerBookCandidatePage {
    pub books: Vec<LayerBookCandidate>,
    pub next_after: Option<i64>,
}

/// Idempotent Layer creation changes composition metadata only, never money.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerCreateReceipt {
    pub layer_id: i64,
    pub replayed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayerReceipt {
    pub term_id: i64,
    pub balance_after_minor: i64,
    pub replayed: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerError {
    Invalid,
    Inaccessible,
    StaleOffer,
    InsufficientFunds,
    ReplayConflict,
    Unavailable,
}
impl LayerCheckoutCommand {
    pub fn validate(&self) -> Result<(), LayerError> {
        if self.wallet.get() != self.actor.get() || self.expected_price.get() <= 0 {
            return Err(LayerError::Invalid);
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn d_05_terms_are_not_automatic_recurring_payments() {
        assert_eq!(LayerTermKind::Prepaid30Days.as_str(), "prepaid_30d");
        assert_eq!(LayerTermKind::Lifetime.as_str(), "lifetime");
    }
}
