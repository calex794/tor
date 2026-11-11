use super::{unavailable, validate_period, AudienceError, PgAudience};
use crate::services::audience::{ClosedReadingCounts, ClosedReadingReport};
use sqlx::Row;

impl PgAudience {
    /// The starts and canonical account/book completions use one read-only MVCC
    /// snapshot and one completed UTC-hour boundary. No identity leaves SQL.
    pub async fn closed_report(&self, days: i32) -> Result<ClosedReadingReport, AudienceError> {
        validate_period(days)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("SELECT set_config('statement_timeout','750ms',true),set_config('lock_timeout','75ms',true)")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        let rows = sqlx::query(include_str!("closed.sql"))
            .bind(days)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?;
        if rows.len() != 2 {
            return Err(AudienceError::Unavailable);
        }
        let counts = |row: &sqlx::postgres::PgRow| -> Result<ClosedReadingCounts, AudienceError> {
            Ok(ClosedReadingCounts {
                starts: row.try_get("starts").map_err(unavailable)?,
                completed_book_pairs: row.try_get("completed_book_pairs").map_err(unavailable)?,
            })
        };
        let report = ClosedReadingReport {
            days,
            measured_at_utc: rows[0].try_get("measured_at_utc").map_err(unavailable)?,
            until_epoch: rows[0].try_get("until_epoch").map_err(unavailable)?,
            current: counts(&rows[0])?,
            previous: counts(&rows[1])?,
        };
        tx.commit().await.map_err(unavailable)?;
        Ok(report)
    }
}
