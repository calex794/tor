use crate::domain::paid::*;
use async_trait::async_trait;
use tor_contracts::{ads::*, exposure::PromotionSlot};

#[async_trait]
pub trait AdConsoleStore: Send + Sync {
    async fn admin_catalog(
        &self,
        a: &AdActor,
    ) -> Result<(Vec<DailyOffer>, Vec<ChapterPackage>), AdError>;
    async fn daily_offers(&self) -> Result<Vec<DailyOffer>, AdError>;
    async fn packages(&self) -> Result<Vec<ChapterPackage>, AdError>;
    async fn owned_books(&self, a: &AdActor) -> Result<Vec<OwnedBook>, AdError>;
    async fn campaigns(
        &self,
        a: &AdActor,
        product: AdProduct,
        before: Option<i64>,
    ) -> Result<Vec<Campaign>, AdError>;
    async fn campaign(&self, a: &AdActor, id: i64) -> Result<Campaign, AdError>;
    async fn publisher(
        &self,
        a: &AdActor,
        before: Option<i64>,
    ) -> Result<(PublisherBalance, Vec<PublisherCredit>), AdError>;
    async fn publisher_settings(
        &self,
        a: &AdActor,
        expected: i64,
        enabled: bool,
    ) -> Result<(), AdError>;
    async fn publish_daily(&self, a: &AdActor, spec: &PublishDaily) -> Result<(i64, i64), AdError>;
    async fn publish_package(
        &self,
        a: &AdActor,
        spec: &PublishPackage,
    ) -> Result<(i64, i64), AdError>;
    async fn slot_policy(&self, a: &AdActor, spec: &SlotPolicy) -> Result<(), AdError>;
    async fn policies(
        &self,
        a: &AdActor,
    ) -> Result<Vec<(String, i64, bool, bool, bool, i32)>, AdError>;
    async fn click(&self, a: &AdActor, token: &str) -> Result<i64, AdError>;
    async fn open(&self, a: &AdActor, token: &str, book: i64) -> Result<(), AdError>;
}
#[async_trait]
pub trait AdDeliveryStore: Send + Sync {
    async fn cards(
        &self,
        a: Option<&AdActor>,
        slot: PromotionSlot,
        host: Option<i64>,
        flags: AdFlags,
        has_editorial: bool,
    ) -> Result<(bool, Vec<PaidCard>), AdError>;
    async fn issue_opportunity(
        &self,
        a: &AdActor,
        assignment: i64,
        chapter: i64,
        token: &str,
    ) -> Result<(), AdError>;
}
