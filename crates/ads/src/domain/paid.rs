use serde::{Deserialize, Serialize};
use tor_contracts::{ads::AdError, exposure::PromotionSlot};

#[derive(Clone, Copy, Default)]
pub struct AdFlags {
    pub daily_purchase: bool,
    pub daily_serving: bool,
    pub chapter_purchase: bool,
    pub planner: bool,
    pub chapter_serving: bool,
    pub billable: bool,
    pub settlement: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct DailyOffer {
    #[serde(serialize_with = "decimal_id")]
    pub id: i64,
    pub revision: i64,
    pub slot: String,
    pub rate_minor: i64,
    pub min_days: i32,
    pub max_days: i32,
    pub terms_version: String,
    pub cancellation_policy: String,
    pub enabled: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct ChapterPackage {
    #[serde(serialize_with = "decimal_id")]
    pub id: i64,
    pub revision: i64,
    pub price_minor: i64,
    pub target_impressions: i64,
    pub publisher_share_bps: i32,
    pub hold_seconds: i32,
    pub daily_cap: i32,
    pub hourly_cap: i32,
    pub session_cap: i32,
    pub terms_version: String,
    pub enabled: bool,
}
#[derive(Clone, Debug, Serialize)]
pub struct Campaign {
    #[serde(serialize_with = "decimal_id")]
    pub id: i64,
    #[serde(serialize_with = "decimal_id")]
    pub book_id: i64,
    pub title: String,
    pub product: String,
    pub status: String,
    pub revision: i64,
    pub budget_minor: i64,
    pub target: i64,
    pub reserved: i64,
    pub reserved_minor: i64,
    pub settled_minor: i64,
    pub refunded_minor: i64,
    pub starts_at: Option<String>,
    pub ends_at: Option<String>,
    pub clicks: i64,
    pub opens: i64,
    pub reads: i64,
    pub library_adds: i64,
    pub purchases: i64,
}
#[derive(Clone, Debug, Serialize)]
pub struct OwnedBook {
    #[serde(serialize_with = "decimal_id")]
    pub id: i64,
    pub title: String,
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct PublisherBalance {
    pub enabled: bool,
    pub revision: i64,
    pub pending_minor: i64,
    pub available_minor: i64,
    pub credited_minor: i64,
}
#[derive(Clone, Debug, Serialize)]
pub struct PublisherCredit {
    #[serde(serialize_with = "decimal_id")]
    pub id: i64,
    pub amount_minor: i64,
    pub status: String,
    pub clear_at: String,
}
#[derive(Clone, Debug)]
pub struct PaidCard {
    pub product: String,
    pub campaign_id: i64,
    pub book_id: i64,
    pub title: String,
    pub generation_id: Option<i64>,
    pub assignment_id: Option<i64>,
    pub opportunity_token: Option<String>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublishDaily {
    pub id: Option<i64>,
    pub expected_revision: i64,
    pub slot: String,
    pub rate_minor: i64,
    pub min_days: i32,
    pub max_days: i32,
    pub terms_version: String,
    pub cancellation_policy: String,
    pub daily_cap: i32,
    pub hourly_cap: i32,
    pub session_cap: i32,
    pub min_interval_seconds: i32,
    pub enabled: bool,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PublishPackage {
    pub id: Option<i64>,
    pub expected_revision: i64,
    pub price_minor: i64,
    pub target_impressions: i64,
    pub publisher_share_bps: i32,
    pub hold_seconds: i32,
    pub daily_cap: i32,
    pub hourly_cap: i32,
    pub session_cap: i32,
    pub terms_version: String,
    pub enabled: bool,
}
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SlotPolicy {
    pub slot: String,
    pub expected_revision: i64,
    pub paid_first: bool,
    pub daily_enabled: bool,
    pub chapter_enabled: bool,
    pub max_cards: i32,
}
pub fn slot(value: &str) -> Result<PromotionSlot, AdError> {
    value.parse().map_err(|_| AdError::Invalid)
}

fn decimal_id<S: serde::Serializer>(id: &i64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&id.to_string())
}
