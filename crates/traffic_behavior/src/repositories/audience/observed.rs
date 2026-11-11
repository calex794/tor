//! Observation-time read model over the existing bounded RIL batch pipeline.
use super::{unavailable, PgFlowAudience};
use crate::services::{
    audience::FlowHourlyError,
    observed_audience::{
        observed_hours, observed_report, validate_zone, ObservedAnchor, ObservedAudienceReport,
        ObservedHour,
    },
};
use sqlx::Row;

impl PgFlowAudience {
    /// Complete UTC-hour intervals, localized for display including DST repeats.
    pub async fn observed_report(
        &self,
        days: i32,
        zone: &str,
    ) -> Result<ObservedAudienceReport, FlowHourlyError> {
        let count = observed_hours(days)?;
        let zone = validate_zone(zone)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("SET LOCAL statement_timeout = '750ms'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        let rows = sqlx::query(include_str!("observed.sql"))
            .bind(count as i32)
            .bind(zone)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?;
        let first = rows.first().ok_or(FlowHourlyError::Unavailable)?;
        let anchor = ObservedAnchor {
            measured_at_utc: first.try_get("measured_at_utc").map_err(unavailable)?,
            until_epoch: first.try_get("until_epoch").map_err(unavailable)?,
            until_utc: first.try_get("until_utc").map_err(unavailable)?,
            current_from_utc: first.try_get("current_from_utc").map_err(unavailable)?,
            previous_from_utc: first.try_get("previous_from_utc").map_err(unavailable)?,
            persisted_discards: first.try_get("discarded").map_err(unavailable)?,
        };
        let mut hours = Vec::with_capacity(rows.len());
        for row in rows {
            hours.push(ObservedHour {
                epoch: row.try_get("hour_epoch").map_err(unavailable)?,
                local_label: row.try_get("local_label").map_err(unavailable)?,
                offset_minutes: row.try_get("offset_minutes").map_err(unavailable)?,
                weekday: usize::try_from(row.try_get::<i32, _>("weekday").map_err(unavailable)?)
                    .map_err(|_| FlowHourlyError::Unavailable)?,
                local_hour: usize::try_from(
                    row.try_get::<i32, _>("local_hour").map_err(unavailable)?,
                )
                .map_err(|_| FlowHourlyError::Unavailable)?,
                views: [
                    row.try_get("views_0").map_err(unavailable)?,
                    row.try_get("views_1").map_err(unavailable)?,
                    row.try_get("views_2").map_err(unavailable)?,
                    row.try_get("views_3").map_err(unavailable)?,
                ],
                sources: [
                    row.try_get("source_0").map_err(unavailable)?,
                    row.try_get("source_1").map_err(unavailable)?,
                    row.try_get("source_2").map_err(unavailable)?,
                    row.try_get("source_3").map_err(unavailable)?,
                ],
            });
        }
        let report = observed_report(days, zone, anchor, hours)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(report)
    }
}
