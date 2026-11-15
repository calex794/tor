//! Bounded current sample and fenced snapshots. No connection spans HTTP work.
mod candidates;
mod report;
mod save;
use crate::domain::public_pages::*;
use sqlx::{PgPool, Postgres, Row, Transaction};

#[derive(Clone)]
pub struct PgPublicPages {
    pool: PgPool,
}
impl PgPublicPages {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    pub async fn claim(&self, origin: &str) -> Result<Option<PageProbeLease>, PageProbeError> {
        if !(8..=255).contains(&origin.len()) {
            return Err(PageProbeError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        limits(&mut tx, false).await?;
        let epoch: Option<i64> = sqlx::query_scalar(
            "WITH candidate AS (
            SELECT singleton FROM flow_public_page_probe WHERE singleton
            AND (lease_until IS NULL OR lease_until<clock_timestamp())
            AND (started_at IS NULL OR started_at<clock_timestamp()-interval '60 seconds')
            FOR UPDATE SKIP LOCKED)
            UPDATE flow_public_page_probe p SET epoch=epoch+1,origin=$1,
              started_at=clock_timestamp(),lease_until=clock_timestamp()+interval '10 seconds'
            FROM candidate c WHERE p.singleton=c.singleton RETURNING epoch",
        )
        .bind(origin)
        .fetch_optional(&mut *tx)
        .await
        .map_err(unavailable)?;
        let Some(epoch) = epoch else {
            return Ok(None);
        };
        let targets = candidates::read(&mut tx, false).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(Some(PageProbeLease {
            epoch,
            origin: origin.into(),
            targets,
        }))
    }
    /// Called immediately before HEAD. The actual public HTTP handler checks the
    /// policy again; final save checks both the live policy and row revisions.
    pub async fn current(&self, target: &PageTarget) -> Result<bool, PageProbeError> {
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        limits(&mut tx, true).await?;
        let live = candidates::read(&mut tx, false).await?;
        tx.commit().await.map_err(unavailable)?;
        Ok(live
            .iter()
            .any(|candidate| candidate == target && candidate.path().is_some()))
    }
}
async fn limits(tx: &mut Transaction<'_, Postgres>, read: bool) -> Result<(), PageProbeError> {
    if read {
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut **tx)
            .await
            .map_err(unavailable)?;
    }
    sqlx::query("SELECT set_config('statement_timeout','750ms',true),set_config('lock_timeout','75ms',true)")
        .execute(&mut **tx).await.map_err(unavailable)?;
    Ok(())
}
fn unavailable(_: sqlx::Error) -> PageProbeError {
    PageProbeError::Unavailable
}
fn target(row: &sqlx::postgres::PgRow) -> Result<PageTarget, PageProbeError> {
    Ok(PageTarget {
        slot: PageSlot::parse(row.try_get("slot").map_err(unavailable)?)?,
        book_id: row.try_get("book_id").map_err(unavailable)?,
        chapter_id: row.try_get("chapter_id").map_err(unavailable)?,
        book_revision: row.try_get("book_revision").map_err(unavailable)?,
        chapter_revision: row.try_get("chapter_revision").map_err(unavailable)?,
        publication_revision: row.try_get("publication_revision").map_err(unavailable)?,
        version_id: row.try_get("version_id").map_err(unavailable)?,
    })
}
