//! Read-only counts from C-owned library and follow rows. No optional analytics.
use super::{unavailable, validate_period, AudienceError, PgAudience};
use crate::services::audience::intent::{IntentCounts, IntentReport};
use sqlx::{postgres::PgRow, Row};

impl PgAudience {
    /// This is one current snapshot. Period cohorts are surviving rows, never
    /// total additions, historical snapshots, retention, or a conversion funnel.
    pub async fn intent_report(&self, days: i32) -> Result<IntentReport, AudienceError> {
        validate_period(days)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("SET LOCAL statement_timeout = '750ms'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        let row = sqlx::query(include_str!("intent.sql"))
            .bind(days)
            .fetch_one(&mut *tx)
            .await
            .map_err(unavailable)?;
        let report = IntentReport {
            days,
            measured_at_utc: row.try_get("measured_at_utc").map_err(unavailable)?,
            current_from_utc: row.try_get("current_from_utc").map_err(unavailable)?,
            previous_from_utc: row.try_get("previous_from_utc").map_err(unavailable)?,
            totals: counts(&row, "")?,
            current: counts(&row, "current_")?,
            previous: counts(&row, "previous_")?,
        };
        tx.commit().await.map_err(unavailable)?;
        Ok(report)
    }
}

fn counts(row: &PgRow, prefix: &str) -> Result<IntentCounts, AudienceError> {
    let value = |name: &str| {
        row.try_get::<i64, _>(format!("{prefix}{name}").as_str())
            .map_err(unavailable)
    };
    Ok(IntentCounts {
        library_links: value("library_links")?,
        library_accounts: value("library_accounts")?,
        follow_links: value("follow_links")?,
        follow_accounts: value("follow_accounts")?,
    })
}
