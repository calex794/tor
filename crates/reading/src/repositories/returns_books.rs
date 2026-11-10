//! The approved F projection enforces current book visibility in this snapshot.
use super::{books_query, unavailable, PgReadingReturns, ReadingReturnsError};
use crate::services::{
    book_returns::{BookReadingReturnsPeriod, BookReadingReturnsReport},
    returns::{validate_period, ReadingReturnsMetric},
};
use sqlx::{postgres::PgRow, Row};

impl PgReadingReturns {
    pub async fn book_returns_report(
        &self,
        book_id: i64,
        days: i32,
    ) -> Result<Option<BookReadingReturnsReport>, ReadingReturnsError> {
        validate_period(days)?;
        if book_id <= 0 {
            return Err(ReadingReturnsError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("SET LOCAL statement_timeout='750ms'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        let rows = sqlx::query(books_query::REPORT)
            .bind(days)
            .bind(book_id)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        if rows.is_empty() {
            return Ok(None);
        }
        if rows.len() != 4 {
            return Err(ReadingReturnsError::Unavailable);
        }
        let first = &rows[0];
        Ok(Some(BookReadingReturnsReport {
            book_id,
            book_title: first.try_get("book_title").map_err(unavailable)?,
            days,
            measured_at_utc: first.try_get("measured_at").map_err(unavailable)?,
            observation_started_at_utc: first.try_get("started_at").map_err(unavailable)?,
            last_observed_at_utc: first.try_get("last_observed_at").map_err(unavailable)?,
            collection_healthy: first.try_get("healthy").map_err(unavailable)?,
            current: period(&rows[..2])?,
            previous: period(&rows[2..])?,
        }))
    }
}

fn period(rows: &[PgRow]) -> Result<BookReadingReturnsPeriod, ReadingReturnsError> {
    let mut metrics = Vec::with_capacity(2);
    for row in rows {
        metrics.push(ReadingReturnsMetric {
            day: row.try_get("day").map_err(unavailable)?,
            eligible: row.try_get("eligible").map_err(unavailable)?,
            returned: row.try_get("returned").map_err(unavailable)?,
            pending: row.try_get("pending").map_err(unavailable)?,
            unknown: row.try_get("unknown").map_err(unavailable)?,
        });
    }
    Ok(BookReadingReturnsPeriod {
        from_utc: rows[0].try_get("from_utc").map_err(unavailable)?,
        until_utc: rows[0].try_get("until_utc").map_err(unavailable)?,
        coverage_complete: rows[0].try_get("coverage_complete").map_err(unavailable)?,
        enrolled: rows[0].try_get("enrolled").map_err(unavailable)?,
        metrics: metrics
            .try_into()
            .map_err(|_| ReadingReturnsError::Unavailable)?,
    })
}
