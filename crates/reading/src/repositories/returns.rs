//! C-owned reading cohorts. No personal rows leave this aggregate repository.
use crate::services::returns::{
    validate_period, ReadingReturnsError, ReadingReturnsMetric, ReadingReturnsPeriod,
    ReadingReturnsReport,
};
use crate::services::{progress::ReadingPlace, sync::SyncError};
use sqlx::{postgres::PgRow, PgPool, Postgres, Row, Transaction};

#[path = "returns_books.rs"]
mod books;
#[path = "returns_books_capture.rs"]
mod books_capture;
#[path = "returns_books_maintenance.rs"]
mod books_maintenance;
#[path = "returns_books_query.rs"]
mod books_query;
#[path = "returns_groups.rs"]
mod groups;

#[path = "returns_capture.rs"]
mod capture;
#[path = "returns_maintenance.rs"]
mod maintenance;
#[path = "returns_query.rs"]
mod query;

pub(super) async fn record(
    tx: &mut Transaction<'_, Postgres>,
    actor: i64,
    book: i64,
    place: ReadingPlace,
    collect_global: bool,
    collect_book: bool,
    collect_flow: bool,
) -> Result<(), SyncError> {
    capture::record(tx, actor, book, place, collect_global).await?;
    books_capture::record(tx, actor, book, place, collect_book).await?;
    super::flow_capture::record_activity(tx, actor, book, place, collect_flow).await
}

#[derive(Clone)]
pub struct PgReadingReturns {
    pool: PgPool,
}

impl PgReadingReturns {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Calendar-day cohort periods, including today; Dn is eligible only after
    /// its complete UTC day and uninterrupted coverage of that target day.
    pub async fn returns_report(
        &self,
        days: i32,
    ) -> Result<ReadingReturnsReport, ReadingReturnsError> {
        validate_period(days)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("SET LOCAL statement_timeout='750ms'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        let rows = sqlx::query(query::REPORT)
            .bind(days)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        if rows.len() != 6 {
            return Err(ReadingReturnsError::Unavailable);
        }
        let first = &rows[0];
        Ok(ReadingReturnsReport {
            days,
            measured_at_utc: first.try_get("measured_at").map_err(unavailable)?,
            observation_started_at_utc: first.try_get("started_at").map_err(unavailable)?,
            last_observed_at_utc: first.try_get("last_observed_at").map_err(unavailable)?,
            collection_healthy: first.try_get("healthy").map_err(unavailable)?,
            current: period(&rows[..3])?,
            previous: period(&rows[3..])?,
        })
    }
}

fn period(rows: &[PgRow]) -> Result<ReadingReturnsPeriod, ReadingReturnsError> {
    let mut metrics = Vec::with_capacity(3);
    for row in rows {
        metrics.push(ReadingReturnsMetric {
            day: row.try_get("day").map_err(unavailable)?,
            eligible: row.try_get("eligible").map_err(unavailable)?,
            returned: row.try_get("returned").map_err(unavailable)?,
            pending: row.try_get("pending").map_err(unavailable)?,
            unknown: row.try_get("unknown").map_err(unavailable)?,
        });
    }
    Ok(ReadingReturnsPeriod {
        from_utc: rows[0].try_get("from_utc").map_err(unavailable)?,
        until_utc: rows[0].try_get("until_utc").map_err(unavailable)?,
        coverage_complete: rows[0].try_get("coverage_complete").map_err(unavailable)?,
        enrolled: rows[0].try_get("enrolled").map_err(unavailable)?,
        metrics: metrics
            .try_into()
            .map_err(|_| ReadingReturnsError::Unavailable)?,
    })
}

fn unavailable(_: sqlx::Error) -> ReadingReturnsError {
    ReadingReturnsError::Unavailable
}
