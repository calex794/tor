//! Current group membership and both cohort periods share one read-only snapshot.
use super::{books, books_query, unavailable, PgReadingReturns, ReadingReturnsError};
use crate::services::group_book_returns::{
    GroupBookReadingReturnsQuery, GroupBookReadingReturnsReport,
};
use sqlx::Row;

impl PgReadingReturns {
    pub async fn group_book_returns_report(
        &self,
        query: &GroupBookReadingReturnsQuery,
    ) -> Result<GroupBookReadingReturnsReport, ReadingReturnsError> {
        // Public DTO fields cannot bypass canonical labels or period bounds.
        let query = GroupBookReadingReturnsQuery::new(query.days, query.kind, query.label.clone())?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("SET LOCAL statement_timeout='750ms'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        let statement = books_query::group_report();
        let rows = sqlx::query(&statement)
            .bind(query.days)
            .bind(query.kind.as_str())
            .bind(query.label.as_deref())
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        if rows.len() != 4 {
            return Err(ReadingReturnsError::Unavailable);
        }
        let first = &rows[0];
        Ok(GroupBookReadingReturnsReport {
            query,
            matched_books: first.try_get("matched_books").map_err(unavailable)?,
            measured_at_utc: first.try_get("measured_at").map_err(unavailable)?,
            observation_started_at_utc: first.try_get("started_at").map_err(unavailable)?,
            last_observed_at_utc: first.try_get("last_observed_at").map_err(unavailable)?,
            collection_healthy: first.try_get("healthy").map_err(unavailable)?,
            current: books::period(&rows[..2])?,
            previous: books::period(&rows[2..])?,
        })
    }
}
