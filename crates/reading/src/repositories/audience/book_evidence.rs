use super::{unavailable, AudienceError, PgAudience};
use crate::services::audience::{
    book_evidence::validate, ClosedBookEvidenceReport, ClosedBookEvidenceRow, ClosedChapterCounts,
};
use sqlx::{postgres::PgRow, Row};

impl PgAudience {
    /// A bounded C-owned aggregate at an explicit shared completed-hour boundary.
    /// No exposure table, analytics profile, or reader identity leaves this port.
    pub async fn closed_book_evidence(
        &self,
        days: i32,
        until_epoch: i64,
        book_ids: &[i64],
    ) -> Result<ClosedBookEvidenceReport, AudienceError> {
        let ids = validate(days, until_epoch, book_ids)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("SELECT set_config('statement_timeout','750ms',true),set_config('lock_timeout','75ms',true)")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        let clock = sqlx::query(
            "SELECT extract(epoch FROM (date_trunc('hour',transaction_timestamp() AT TIME ZONE 'UTC')
                AT TIME ZONE 'UTC'))::bigint AS latest,
             to_char(transaction_timestamp() AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS measured_at_utc",
        )
        .fetch_one(&mut *tx)
        .await
        .map_err(unavailable)?;
        let latest: i64 = clock.try_get("latest").map_err(unavailable)?;
        // The previous hour permits a settled upstream window and a boundary
        // crossing during one bounded request, never arbitrary historical reads.
        if until_epoch != latest && until_epoch != latest - 3600 {
            return Err(AudienceError::Invalid);
        }
        let records = sqlx::query(include_str!("book_evidence.sql"))
            .bind(until_epoch)
            .bind(days)
            .bind(&ids)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?;
        if records.len() != ids.len() * 2 {
            return Err(AudienceError::Unavailable);
        }
        let mut rows = Vec::with_capacity(ids.len());
        let pairs = records.as_chunks::<2>().0;
        for (id, pair) in ids.into_iter().zip(pairs) {
            if pair[0].try_get::<i64, _>("book_id").map_err(unavailable)? != id
                || pair[1].try_get::<i64, _>("book_id").map_err(unavailable)? != id
                || pair[0].try_get::<i32, _>("ordinal").map_err(unavailable)? != 0
                || pair[1].try_get::<i32, _>("ordinal").map_err(unavailable)? != 1
            {
                return Err(AudienceError::Unavailable);
            }
            rows.push(ClosedBookEvidenceRow {
                book_id: id,
                current: period(&pair[0])?,
                previous: period(&pair[1])?,
            });
        }
        let report = ClosedBookEvidenceReport {
            days,
            until_epoch,
            measured_at_utc: clock.try_get("measured_at_utc").map_err(unavailable)?,
            rows,
        };
        tx.commit().await.map_err(unavailable)?;
        Ok(report)
    }
}

fn period(row: &PgRow) -> Result<Option<ClosedChapterCounts>, AudienceError> {
    let eligible: bool = row.try_get("eligible").map_err(unavailable)?;
    let unmatched: i64 = row.try_get("unmatched").map_err(unavailable)?;
    let counts = ClosedChapterCounts {
        starts: row.try_get("starts").map_err(unavailable)?,
        chapter_completions: row.try_get("chapter_completions").map_err(unavailable)?,
    };
    if unmatched < 0 || counts.starts < 0 || counts.chapter_completions < 0 {
        return Err(AudienceError::Unavailable);
    }
    Ok((eligible && unmatched == 0).then_some(counts))
}
