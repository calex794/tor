use crate::domain::exposure::ExposureError;
use sqlx::{PgPool, Postgres, Transaction};
use std::time::Duration;

mod checkpoint;
mod lifecycle;
mod read;

#[derive(Clone)]
pub struct PgExposureHealth {
    pub(super) pool: PgPool,
}
impl PgExposureHealth {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
async fn bounded<T>(
    work: impl std::future::Future<Output = Result<T, ExposureError>>,
) -> Result<T, ExposureError> {
    tokio::time::timeout(Duration::from_millis(950), work)
        .await
        .map_err(|_| ExposureError::Unavailable)?
}
async fn settings(tx: &mut Transaction<'_, Postgres>) -> Result<(), ExposureError> {
    sqlx::query("SELECT set_config('statement_timeout','750ms',true),set_config('lock_timeout','75ms',true)")
        .execute(&mut **tx).await?;
    Ok(())
}
