use super::{unavailable, AudienceError, PgAudience};
use crate::services::audience::time::{
    validate, ReadingTimeCell, ReadingTimeCounts, ReadingTimeHour, ReadingTimePeriod,
    ReadingTimeReport,
};
use sqlx::{postgres::PgRow, Row};

impl PgAudience {
    /// Numerical retained C evidence only: no identity or content leaves SQL.
    pub async fn time_report(
        &self,
        days: i32,
        zone: &str,
    ) -> Result<ReadingTimeReport, AudienceError> {
        validate(days, zone)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("SELECT set_config('statement_timeout','750ms',true),set_config('lock_timeout','75ms',true)")
            .execute(&mut *tx).await.map_err(unavailable)?;
        let rows = sqlx::query(include_str!("time.sql"))
            .bind(days)
            .bind(zone)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?;
        let length = days as usize * 24;
        if rows.len() != length * 2 {
            return Err(AudienceError::Unavailable);
        }
        let current = period(&rows[..length], 0)?;
        let previous = period(&rows[length..], 1)?;
        let until_epoch = rows[0].try_get("until_epoch").map_err(unavailable)?;
        if current.until_epoch != until_epoch || previous.until_epoch != current.from_epoch {
            return Err(AudienceError::Unavailable);
        }
        let report = ReadingTimeReport {
            days,
            time_zone: zone.into(),
            until_epoch,
            current,
            previous,
            measured_at_utc: rows[0].try_get("measured_at_utc").map_err(unavailable)?,
        };
        tx.commit().await.map_err(unavailable)?;
        Ok(report)
    }
}

fn period(rows: &[PgRow], ordinal: i32) -> Result<ReadingTimePeriod, AudienceError> {
    let first = rows.first().ok_or(AudienceError::Unavailable)?;
    let mut period = ReadingTimePeriod {
        from_epoch: first.try_get("from_epoch").map_err(unavailable)?,
        until_epoch: first.try_get("period_until_epoch").map_err(unavailable)?,
        from_utc: first.try_get("from_utc").map_err(unavailable)?,
        until_utc: first.try_get("until_utc").map_err(unavailable)?,
        totals: None,
        hours: Vec::with_capacity(rows.len()),
        by_hour: [ReadingTimeCell::default(); 24],
        by_weekday: [ReadingTimeCell::default(); 7],
    };
    for row in rows {
        if row.try_get::<i32, _>("ordinal").map_err(unavailable)? != ordinal {
            return Err(AudienceError::Unavailable);
        }
        let unmatched: i64 = row.try_get("unmatched").map_err(unavailable)?;
        if unmatched < 0 {
            return Err(AudienceError::Unavailable);
        }
        let counts = ReadingTimeCounts {
            starts: row.try_get("starts").map_err(unavailable)?,
            chapter_completions: row.try_get("chapter_completions").map_err(unavailable)?,
        };
        let weekday = row.try_get::<i32, _>("weekday").map_err(unavailable)?;
        let local_hour = row.try_get::<i32, _>("local_hour").map_err(unavailable)?;
        period.hours.push(ReadingTimeHour {
            epoch: row.try_get("epoch").map_err(unavailable)?,
            local_label: row.try_get("local_label").map_err(unavailable)?,
            offset_minutes: row.try_get("offset_minutes").map_err(unavailable)?,
            weekday: usize::try_from(weekday).map_err(|_| AudienceError::Unavailable)?,
            local_hour: usize::try_from(local_hour).map_err(|_| AudienceError::Unavailable)?,
            counts: (unmatched == 0).then_some(counts),
        });
    }
    period.summarize()?;
    Ok(period)
}
