//! D/S035: a deterministic all-or-nothing quote for selected paid chapters.
//! No external payment, creator earnings, refund, or public route is implied.
use crate::domain::money::{CheckoutError, IdempotencyKey, MoneyMinor, PurchaseReceipt, WalletId};
use sha2::{Digest, Sha256};
use tor_contracts::paid_delivery::{ChapterVersionRef, PositiveId};

pub const MAX_BATCH_CHAPTERS: usize = 20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BatchChapterOffer {
    pub edition: ChapterVersionRef,
    pub expected_price: MoneyMinor,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BatchCheckoutError {
    InvalidBatch,
    MixedBooks,
    Checkout(CheckoutError),
}

impl From<CheckoutError> for BatchCheckoutError {
    fn from(error: CheckoutError) -> Self {
        Self::Checkout(error)
    }
}

/// The sorted, immutable quote is hashed independently of JSON order.
/// Callers cannot change one chapter after the idempotency key is claimed.
#[derive(Clone, Debug)]
pub struct BatchPurchaseCommand {
    actor_id: PositiveId,
    wallet_id: WalletId,
    offers: Vec<BatchChapterOffer>,
    idempotency_key: IdempotencyKey,
    request_sha256: [u8; 32],
    total_price: MoneyMinor,
}

impl BatchPurchaseCommand {
    pub fn new(
        actor_id: PositiveId,
        wallet_id: WalletId,
        mut offers: Vec<BatchChapterOffer>,
        idempotency_key: IdempotencyKey,
    ) -> Result<Self, BatchCheckoutError> {
        if wallet_id.get() != actor_id.get() || !(2..=MAX_BATCH_CHAPTERS).contains(&offers.len()) {
            return Err(BatchCheckoutError::InvalidBatch);
        }
        offers.sort_by_key(|offer| offer.edition.chapter_id.get());
        if offers
            .windows(2)
            .any(|w| w[0].edition.chapter_id == w[1].edition.chapter_id)
        {
            return Err(BatchCheckoutError::InvalidBatch);
        }
        let mut total = MoneyMinor::new(0).expect("zero is valid");
        let mut hash = Sha256::new();
        hash.update(b"tor.chapter.batch.v1");
        hash.update(actor_id.get().to_be_bytes());
        hash.update(wallet_id.get().to_be_bytes());
        hash.update((offers.len() as u64).to_be_bytes());
        for offer in &offers {
            if offer.expected_price.get() == 0 {
                return Err(BatchCheckoutError::InvalidBatch);
            }
            total = total
                .checked_add(offer.expected_price)
                .map_err(|_| BatchCheckoutError::InvalidBatch)?;
            for value in [
                offer.edition.chapter_id.get(),
                offer.edition.version_id.get(),
                offer.expected_price.get(),
            ] {
                hash.update(value.to_be_bytes());
            }
        }
        Ok(Self {
            actor_id,
            wallet_id,
            offers,
            idempotency_key,
            request_sha256: hash.finalize().into(),
            total_price: total,
        })
    }

    pub fn actor(&self) -> PositiveId {
        self.actor_id
    }

    pub fn wallet(&self) -> WalletId {
        self.wallet_id
    }

    pub fn offers(&self) -> &[BatchChapterOffer] {
        &self.offers
    }

    pub fn key(&self) -> &IdempotencyKey {
        &self.idempotency_key
    }

    pub fn request_sha256(&self) -> &[u8; 32] {
        &self.request_sha256
    }

    pub fn total(&self) -> MoneyMinor {
        self.total_price
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BatchPurchaseReceipt {
    pub purchases: Vec<PurchaseReceipt>,
    pub total: MoneyMinor,
    pub balance_after: MoneyMinor,
    pub replayed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(chapter: i64, version: i64, price: i64) -> BatchChapterOffer {
        BatchChapterOffer {
            edition: ChapterVersionRef {
                chapter_id: PositiveId::new(chapter).unwrap(),
                version_id: PositiveId::new(version).unwrap(),
            },
            expected_price: MoneyMinor::new(price).unwrap(),
        }
    }

    fn cmd(offers: Vec<BatchChapterOffer>) -> Result<BatchPurchaseCommand, BatchCheckoutError> {
        BatchPurchaseCommand::new(
            PositiveId::new(7).unwrap(),
            WalletId::new(7).unwrap(),
            offers,
            IdempotencyKey::parse(&format!("{}8", "A".repeat(42))).unwrap(),
        )
    }

    #[test]
    fn d_s035_quote_is_canonical_independent_of_selected_order() {
        let a = cmd(vec![offer(12, 120, 450), offer(11, 110, 250)]).unwrap();
        let b = cmd(vec![offer(11, 110, 250), offer(12, 120, 450)]).unwrap();
        assert_eq!(a.offers(), b.offers());
        assert_eq!(a.request_sha256(), b.request_sha256());
        assert_eq!(a.total().get(), 700);
        let changed = cmd(vec![offer(11, 110, 251), offer(12, 120, 450)]).unwrap();
        assert_ne!(a.request_sha256(), changed.request_sha256());
    }

    #[test]
    fn d_s035_rejects_duplicate_underpriced_or_overflowing_quotes() {
        assert!(matches!(cmd(vec![]), Err(BatchCheckoutError::InvalidBatch)));
        assert!(matches!(
            cmd(vec![offer(1, 1, 1)]),
            Err(BatchCheckoutError::InvalidBatch)
        ));
        assert!(matches!(
            cmd(vec![offer(1, 1, 1), offer(1, 2, 1)]),
            Err(BatchCheckoutError::InvalidBatch)
        ));
        assert!(matches!(
            cmd(vec![offer(1, 1, 0), offer(2, 2, 1)]),
            Err(BatchCheckoutError::InvalidBatch)
        ));
        assert!(matches!(
            cmd(vec![offer(1, 1, i64::MAX), offer(2, 2, 1)]),
            Err(BatchCheckoutError::InvalidBatch)
        ));
        let too_many = (1..=21).map(|n| offer(n, n, 1)).collect();
        assert!(matches!(
            cmd(too_many),
            Err(BatchCheckoutError::InvalidBatch)
        ));
    }
}
