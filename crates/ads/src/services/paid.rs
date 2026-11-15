//! F use cases own feature gates; all financial effects cross the D port.
use crate::domain::paid::AdFlags;
use tor_contracts::ads::*;

pub struct PaidAds<'a> {
    pub funding: &'a dyn AdFundingPort,
    pub flags: AdFlags,
}
impl PaidAds<'_> {
    pub async fn purchase(&self, c: &AdPurchaseCommand) -> Result<AdPurchaseReceipt, AdError> {
        let enabled = match c.purchase.product() {
            AdProduct::Daily => self.flags.daily_purchase,
            AdProduct::Chapter => self.flags.chapter_purchase,
        };
        if !enabled {
            return Err(AdError::NotReady);
        }
        self.funding.purchase(c).await
    }
    pub async fn impression(&self, c: &AdImpressionClaim) -> Result<AdReservation, AdError> {
        if !self.flags.billable {
            return Err(AdError::NotReady);
        }
        self.funding.reserve(c).await
    }
    // Existing funded obligations must still be refundable when purchase/serving
    // is switched off. Emergency feature switches never trap customer escrow.
    pub async fn cancel(&self, c: &AdCancelCommand) -> Result<AdCancellation, AdError> {
        self.funding.cancel(c).await
    }
}
