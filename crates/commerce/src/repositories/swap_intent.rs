//! D-06: immutable intent identity for a future atomic direct chapter swap.
//! This module does NOT execute money movements or grant access.
//! Never accept a UI quote as settlement authorization.

use sha2::{Digest, Sha256};

/// Exact transaction intent. Prices and the target edition must be checked
/// again under PostgreSQL locks before posting a swap.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DirectSwapIntent {
    pub buyer_id: i64,
    pub source_purchase_id: i64,
    pub target_chapter_id: i64,
    pub target_version_id: i64,
    pub source_price_minor: i64,
    pub target_price_minor: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectSwapIntentError {
    InvalidIdentity,
    InvalidMoney,
    Overflow,
    InsufficientBalance,
}

impl DirectSwapIntent {
    /// A stable, domain-separated digest for durable idempotency claims.
    /// The request key must be unique per buyer; reuse with a different digest
    /// must fail, not silently redirect to another target.
    pub fn digest(self) -> Result<[u8; 32], DirectSwapIntentError> {
        self.validate()?;
        let mut hash = Sha256::new();
        hash.update(b"tor:finance:direct-swap-intent:v1");
        for value in [
            self.buyer_id,
            self.source_purchase_id,
            self.target_chapter_id,
            self.target_version_id,
            self.source_price_minor,
            self.target_price_minor,
        ] {
            hash.update(value.to_be_bytes());
        }
        Ok(hash.finalize().into())
    }

    pub fn validate(self) -> Result<(), DirectSwapIntentError> {
        if self.buyer_id <= 0
            || self.source_purchase_id <= 0
            || self.target_chapter_id <= 0
            || self.target_version_id <= 0
        {
            return Err(DirectSwapIntentError::InvalidIdentity);
        }
        if self.source_price_minor <= 0 || self.target_price_minor <= 0 {
            return Err(DirectSwapIntentError::InvalidMoney);
        }
        Ok(())
    }

    /// Positive means buyer owes more; negative means buyer receives credit.
    /// This is arithmetic validation only, never a permission to post.
    pub fn delta(self, wallet_balance_minor: i64) -> Result<i64, DirectSwapIntentError> {
        self.validate()?;
        if wallet_balance_minor < 0 {
            return Err(DirectSwapIntentError::InvalidMoney);
        }
        let delta = self
            .target_price_minor
            .checked_sub(self.source_price_minor)
            .ok_or(DirectSwapIntentError::Overflow)?;
        if delta > wallet_balance_minor {
            return Err(DirectSwapIntentError::InsufficientBalance);
        }
        Ok(delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> DirectSwapIntent {
        DirectSwapIntent {
            buyer_id: 11,
            source_purchase_id: 22,
            target_chapter_id: 33,
            target_version_id: 44,
            source_price_minor: 100,
            target_price_minor: 150,
        }
    }

    #[test]
    fn d06_swap_intent_digest_is_stable_and_binds_all_fields() {
        let intent = sample();
        let expected = intent.digest().unwrap();
        assert_eq!(expected, intent.digest().unwrap());
        let changed = [
            DirectSwapIntent {
                buyer_id: 12,
                ..intent
            },
            DirectSwapIntent {
                source_purchase_id: 23,
                ..intent
            },
            DirectSwapIntent {
                target_chapter_id: 34,
                ..intent
            },
            DirectSwapIntent {
                target_version_id: 45,
                ..intent
            },
            DirectSwapIntent {
                source_price_minor: 101,
                ..intent
            },
            DirectSwapIntent {
                target_price_minor: 151,
                ..intent
            },
        ];
        for candidate in changed {
            assert_ne!(candidate.digest().unwrap(), expected);
        }
    }

    #[test]
    fn d06_swap_intent_rejects_bad_identifiers_and_money() {
        let intent = sample();
        assert_eq!(
            DirectSwapIntent {
                buyer_id: 0,
                ..intent
            }
            .digest(),
            Err(DirectSwapIntentError::InvalidIdentity)
        );
        assert_eq!(
            DirectSwapIntent {
                target_version_id: -1,
                ..intent
            }
            .digest(),
            Err(DirectSwapIntentError::InvalidIdentity)
        );
        assert_eq!(
            DirectSwapIntent {
                target_price_minor: 0,
                ..intent
            }
            .digest(),
            Err(DirectSwapIntentError::InvalidMoney)
        );
    }

    #[test]
    fn d06_swap_intent_delta_never_overdraws() {
        let intent = sample();
        assert_eq!(intent.delta(50), Ok(50));
        assert_eq!(
            intent.delta(49),
            Err(DirectSwapIntentError::InsufficientBalance)
        );
        assert_eq!(intent.delta(-1), Err(DirectSwapIntentError::InvalidMoney));
        assert_eq!(
            DirectSwapIntent {
                target_price_minor: 80,
                ..intent
            }
            .delta(0),
            Ok(-20)
        );
        assert_eq!(
            DirectSwapIntent {
                target_price_minor: 100,
                ..intent
            }
            .delta(0),
            Ok(0)
        );
    }
}
