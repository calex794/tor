//! Single read-only snapshot over C facts and the already approved public catalogue port.
mod rank;

use super::{unavailable, AudienceError, PgAudience};
use crate::services::audience::canonical::{
    CanonicalCounts, CanonicalDimension, CanonicalQuery, CanonicalReport, CanonicalRow,
    CANONICAL_PAGE_SIZE,
};
use sqlx::{postgres::PgRow, Row};

impl PgAudience {
    pub async fn canonical_report(
        &self,
        query: &CanonicalQuery,
    ) -> Result<CanonicalReport, AudienceError> {
        let query = query.clone().validated()?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("SET LOCAL statement_timeout = '750ms'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        let records = sqlx::query(include_str!("report.sql"))
            .bind(query.days)
            .bind(&query.genre)
            .bind(&query.tag)
            .bind(query.without_genre)
            .bind(query.without_tag)
            .bind(query.dimension.key())
            .bind(query.after_id)
            .bind(&query.after_label)
            .bind((CANONICAL_PAGE_SIZE + 1) as i64)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?;
        let total = records.first().ok_or(AudienceError::Unavailable)?;
        if !total.try_get::<bool, _>("is_total").map_err(unavailable)? {
            return Err(AudienceError::Unavailable);
        }
        let mut rows = records
            .iter()
            .skip(1)
            .map(|row| {
                Ok(CanonicalRow {
                    book_id: row.try_get("book_id").map_err(unavailable)?,
                    label: row.try_get("label").map_err(unavailable)?,
                    counts: counts(row)?,
                })
            })
            .collect::<Result<Vec<_>, AudienceError>>()?;
        let more = rows.len() > CANONICAL_PAGE_SIZE;
        rows.truncate(CANONICAL_PAGE_SIZE);
        let (next_after_id, next_after_label) = if more {
            let last = rows.last().ok_or(AudienceError::Unavailable)?;
            match query.dimension {
                CanonicalDimension::Books => {
                    (Some(last.book_id.ok_or(AudienceError::Unavailable)?), None)
                }
                _ => (
                    None,
                    Some(last.label.clone().ok_or(AudienceError::Unavailable)?),
                ),
            }
        } else {
            (None, None)
        };
        let report = CanonicalReport {
            measured_at_utc: total.try_get("measured_at_utc").map_err(unavailable)?,
            current_from_utc: total.try_get("current_from_utc").map_err(unavailable)?,
            previous_from_utc: total.try_get("previous_from_utc").map_err(unavailable)?,
            totals: counts(total)?,
            rows,
            next_after_id,
            next_after_label,
        };
        tx.commit().await.map_err(unavailable)?;
        Ok(report)
    }
}
fn counts(row: &PgRow) -> Result<CanonicalCounts, AudienceError> {
    let result = CanonicalCounts {
        books: row.try_get("books").map_err(unavailable)?,
        current_pairs: row.try_get("current_pairs").map_err(unavailable)?,
        previous_pairs: row.try_get("previous_pairs").map_err(unavailable)?,
        library_links: row.try_get("library_links").map_err(unavailable)?,
        follow_links: row.try_get("follow_links").map_err(unavailable)?,
    };
    if [
        result.books,
        result.current_pairs,
        result.previous_pairs,
        result.library_links,
        result.follow_links,
    ]
    .into_iter()
    .any(|value| value < 0)
    {
        return Err(AudienceError::Unavailable);
    }
    Ok(result)
}
