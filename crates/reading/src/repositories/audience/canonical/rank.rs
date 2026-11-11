use super::{counts, unavailable, AudienceError, PgAudience};
use crate::services::audience::canonical::{
    CanonicalQuery, CanonicalRankReport, CanonicalRankRow, CANONICAL_PAGE_SIZE,
    COMPARISON_MIN_PAIRS,
};
use sqlx::Row;

impl PgAudience {
    /// Full eligible scope is grouped before ranking. The existing keyset report
    /// remains an independent path; a rank query cannot silently discard a cursor.
    pub async fn canonical_rank_report(
        &self,
        query: &CanonicalQuery,
    ) -> Result<CanonicalRankReport, AudienceError> {
        let query = query.clone().validated()?;
        if query.after_id.is_some() || query.after_label.is_some() {
            return Err(AudienceError::Invalid);
        }
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("SELECT set_config('statement_timeout','750ms',true),set_config('lock_timeout','75ms',true)")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        let records = sqlx::query(include_str!("rank.sql"))
            .bind(query.days)
            .bind(&query.genre)
            .bind(&query.tag)
            .bind(query.without_genre)
            .bind(query.without_tag)
            .bind(query.dimension.key())
            .bind(CANONICAL_PAGE_SIZE as i64)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?;
        let total = records.first().ok_or(AudienceError::Unavailable)?;
        if !total.try_get::<bool, _>("is_total").map_err(unavailable)?
            || records.len() > CANONICAL_PAGE_SIZE + 1
        {
            return Err(AudienceError::Unavailable);
        }
        let rows = records.iter().skip(1).map(|row| {
            let counts = counts(row)?;
            Ok(CanonicalRankRow {
                book_id: row.try_get("book_id").map_err(unavailable)?,
                label: row.try_get("label").map_err(unavailable)?,
                small_current_sample: counts.current_pairs < COMPARISON_MIN_PAIRS,
                small_previous_sample: counts.previous_pairs < COMPARISON_MIN_PAIRS,
                counts,
            })
        }).collect::<Result<Vec<_>, AudienceError>>()?;
        let report = CanonicalRankReport {
            measured_at_utc: total.try_get("measured_at_utc").map_err(unavailable)?,
            current_from_utc: total.try_get("current_from_utc").map_err(unavailable)?,
            previous_from_utc: total.try_get("previous_from_utc").map_err(unavailable)?,
            totals: counts(total)?,
            rows,
        };
        tx.commit().await.map_err(unavailable)?;
        Ok(report)
    }
}
