//! Anonymous aggregate owner. Short-lived nonce state is unrelated to retired REC subjects.
use crate::domain::exposure::*;
use sqlx::{PgPool, Row};
use std::str::FromStr;
use tor_contracts::exposure::{
    CardObservation, EditorialPlacement, ExposureSurface, PromotionSlot,
};
mod health;
pub use health::PgExposureHealth;
mod readiness;
mod report;
mod write;

#[derive(Clone)]
pub struct PgExposure {
    pool: PgPool,
}
impl PgExposure {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
impl From<sqlx::Error> for ExposureError {
    fn from(_: sqlx::Error) -> Self {
        Self::Unavailable
    }
}
fn card(row: &sqlx::postgres::PgRow) -> Result<CardObservation, ExposureError> {
    let surface = ExposureSurface::from_str(row.try_get("surface")?)
        .map_err(|_| ExposureError::Unavailable)?;
    let campaign_id: i64 = row.try_get("campaign_id")?;
    Ok(CardObservation {
        book_id: row.try_get("book_id")?,
        surface,
        editorial: if campaign_id == 0 {
            None
        } else {
            Some(EditorialPlacement {
                campaign_id,
                revision: row.try_get("campaign_revision")?,
                slot: PromotionSlot::from_str(row.try_get("slot")?)
                    .map_err(|_| ExposureError::Unavailable)?,
            })
        },
    })
}
fn counts(row: &sqlx::postgres::PgRow, prefix: &str) -> Result<Counts, ExposureError> {
    Ok(Counts {
        opportunities: row.try_get(format!("{prefix}opportunities").as_str())?,
        rendered: row.try_get(format!("{prefix}rendered").as_str())?,
        views: row.try_get(format!("{prefix}views").as_str())?,
        clicks: row.try_get(format!("{prefix}clicks").as_str())?,
        expired_unobserved: row.try_get(format!("{prefix}expired_unobserved").as_str())?,
        replayed: row.try_get(format!("{prefix}replayed").as_str())?,
    })
}
