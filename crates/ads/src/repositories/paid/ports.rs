use super::*;
use crate::services::console::{AdConsoleStore, AdDeliveryStore};
use async_trait::async_trait;
use tor_contracts::{ads::AdProduct, exposure::PromotionSlot};

#[async_trait]
impl AdConsoleStore for PgPaidAds {
    async fn admin_catalog(
        &self,
        a: &AdActor,
    ) -> Result<(Vec<DailyOffer>, Vec<ChapterPackage>), AdError> {
        PgPaidAds::admin_catalog(self, a).await
    }
    async fn daily_offers(&self) -> Result<Vec<DailyOffer>, AdError> {
        PgPaidAds::daily_offers(self).await
    }
    async fn packages(&self) -> Result<Vec<ChapterPackage>, AdError> {
        PgPaidAds::packages(self).await
    }
    async fn owned_books(&self, a: &AdActor) -> Result<Vec<OwnedBook>, AdError> {
        PgPaidAds::owned_books(self, a).await
    }
    async fn campaigns(
        &self,
        a: &AdActor,
        product: AdProduct,
        before: Option<i64>,
    ) -> Result<Vec<Campaign>, AdError> {
        PgPaidAds::campaigns(self, a, product, before).await
    }
    async fn campaign(&self, a: &AdActor, id: i64) -> Result<Campaign, AdError> {
        PgPaidAds::campaign(self, a, id).await
    }
    async fn publisher(
        &self,
        a: &AdActor,
        before: Option<i64>,
    ) -> Result<(PublisherBalance, Vec<PublisherCredit>), AdError> {
        PgPaidAds::publisher(self, a, before).await
    }
    async fn publisher_settings(
        &self,
        a: &AdActor,
        expected: i64,
        enabled: bool,
    ) -> Result<(), AdError> {
        PgPaidAds::publisher_settings(self, a, expected, enabled).await
    }
    async fn publish_daily(&self, a: &AdActor, spec: &PublishDaily) -> Result<(i64, i64), AdError> {
        PgPaidAds::publish_daily(self, a, spec).await
    }
    async fn publish_package(
        &self,
        a: &AdActor,
        spec: &PublishPackage,
    ) -> Result<(i64, i64), AdError> {
        PgPaidAds::publish_package(self, a, spec).await
    }
    async fn slot_policy(&self, a: &AdActor, spec: &SlotPolicy) -> Result<(), AdError> {
        PgPaidAds::slot_policy(self, a, spec).await
    }
    async fn policies(
        &self,
        a: &AdActor,
    ) -> Result<Vec<(String, i64, bool, bool, bool, i32)>, AdError> {
        PgPaidAds::policies(self, a).await
    }
    async fn click(&self, a: &AdActor, token: &str) -> Result<i64, AdError> {
        PgPaidAds::click(self, a, token).await
    }
    async fn open(&self, a: &AdActor, token: &str, book: i64) -> Result<(), AdError> {
        PgPaidAds::open(self, a, token, book).await
    }
}
#[async_trait]
impl AdDeliveryStore for PgPaidAds {
    async fn cards(
        &self,
        a: Option<&AdActor>,
        slot: PromotionSlot,
        host: Option<i64>,
        flags: AdFlags,
        has_editorial: bool,
    ) -> Result<(bool, Vec<PaidCard>), AdError> {
        PgPaidAds::cards(self, a, slot, host, flags, has_editorial).await
    }
    async fn issue_opportunity(
        &self,
        a: &AdActor,
        assignment: i64,
        chapter: i64,
        token: &str,
    ) -> Result<(), AdError> {
        PgPaidAds::issue_opportunity(self, a, assignment, chapter, token).await
    }
}
