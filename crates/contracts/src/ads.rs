//! F/D paid advertising boundary. No SQL, HTTP, float money or sibling imports.
use crate::{exposure::PromotionSlot, money::MoneyMinor};
use async_trait::async_trait;

macro_rules! positive_id {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
        pub struct $name(i64);
        impl $name {
            pub fn new(value: i64) -> Result<Self, AdError> {
                (value > 0).then_some(Self(value)).ok_or(AdError::Invalid)
            }
            pub const fn get(self) -> i64 {
                self.0
            }
        }
    };
}
positive_id!(AdCampaignId);
positive_id!(AdOfferId);
positive_id!(AdGenerationId);
positive_id!(AdAssignmentId);
positive_id!(AdImpressionId);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdProduct {
    Daily,
    Chapter,
}
impl AdProduct {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Daily => "daily",
            Self::Chapter => "chapter",
        }
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AdError {
    Invalid,
    Denied,
    NotFound,
    Conflict,
    IdempotencyConflict,
    Insufficient,
    CapReached,
    Expired,
    NotReady,
    Unavailable,
}
#[derive(Clone)]
pub struct AdActor {
    pub user_id: i64,
    pub session_digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DailyPurchase {
    pub book_id: i64,
    pub offer_id: i64,
    pub offer_revision: i64,
    pub placement: PromotionSlot,
    pub starts_at: String,
    pub ends_at: String,
    pub total_days: u32,
    pub prepaid_days: u32,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChapterPurchase {
    pub book_id: i64,
    pub package_id: i64,
    pub expected_package_revision: i64,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdPurchase {
    Daily(DailyPurchase),
    Chapter(ChapterPurchase),
}
impl AdPurchase {
    pub const fn product(&self) -> AdProduct {
        match self {
            Self::Daily(_) => AdProduct::Daily,
            Self::Chapter(_) => AdProduct::Chapter,
        }
    }
    pub const fn book_id(&self) -> i64 {
        match self {
            Self::Daily(v) => v.book_id,
            Self::Chapter(v) => v.book_id,
        }
    }
}
#[derive(Clone)]
pub struct AdPurchaseCommand {
    pub actor: AdActor,
    pub command_key: String,
    pub purchase: AdPurchase,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdPurchaseReceipt {
    pub campaign_id: AdCampaignId,
    pub product: AdProduct,
    pub charged: MoneyMinor,
    pub balance_after: MoneyMinor,
    pub replayed: bool,
}
#[derive(Clone)]
pub struct AdImpressionClaim {
    pub actor: AdActor,
    pub context: AdDeliveryContext,
    pub viewability_token: String,
    pub visible_percent: u16,
    pub duration_ms: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdDeliveryContext {
    pub campaign_id: AdCampaignId,
    pub generation_id: AdGenerationId,
    pub assignment_id: AdAssignmentId,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdReservation {
    pub impression_id: AdImpressionId,
    pub campaign_id: AdCampaignId,
    pub ordinal: i64,
    pub gross: MoneyMinor,
    pub replayed: bool,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdCancellation {
    pub campaign_id: AdCampaignId,
    pub refunded: MoneyMinor,
    pub replayed: bool,
    pub pending: bool,
}
#[derive(Clone)]
pub struct AdCancelCommand {
    pub actor: AdActor,
    pub product: AdProduct,
    pub campaign_id: AdCampaignId,
    pub expected_revision: i64,
    pub command_key: String,
    pub reason: String,
}

/// D owns every wallet/escrow/settlement mutation. Its transaction may update
/// the explicitly documented F campaign/reservation rows for the same effect.
#[async_trait]
pub trait AdFundingPort: Send + Sync {
    async fn purchase(&self, command: &AdPurchaseCommand) -> Result<AdPurchaseReceipt, AdError>;
    async fn reserve(&self, claim: &AdImpressionClaim) -> Result<AdReservation, AdError>;
    async fn cancel(&self, command: &AdCancelCommand) -> Result<AdCancellation, AdError>;
    async fn settle(&self, campaign: AdCampaignId, limit: u32) -> Result<u32, AdError>;
    async fn mature_publisher(&self, actor: &AdActor, limit: u32) -> Result<MoneyMinor, AdError>;
    async fn release_publisher(
        &self,
        actor: &AdActor,
        command_key: &str,
        limit: u32,
    ) -> Result<MoneyMinor, AdError>;
}
