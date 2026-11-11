use super::*;
use crate::domain::exposure_health::{ExposureHealth, ExposureHealthPeriod};
use sqlx::Row;

impl PgExposureHealth {
    pub async fn report(&self, days: i32, until_epoch: i64) -> Result<ExposureHealth, ExposureError> {
        bounded(async {
            let mut tx = self.pool.begin().await?;
            sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
                .execute(&mut *tx).await?;
            settings(&mut tx).await?;
            let report = Self::report_in(&mut tx, days, until_epoch).await?;
            tx.commit().await?;
            Ok(report)
        }).await
    }

    /// Shared by the counts owner inside its single repeatable-read snapshot.
    /// The supplied boundary is the counts boundary, not the time of this read.
    pub(in crate::repositories::exposure) async fn report_in(
        tx: &mut Transaction<'_, Postgres>,
        days: i32,
        until_epoch: i64,
    ) -> Result<ExposureHealth, ExposureError> {
        if ![1, 7, 30, 90].contains(&days) || until_epoch < 0 || until_epoch % 3600 != 0 {
            return Err(ExposureError::Invalid);
        }
        let valid: bool = sqlx::query_scalar(
            "SELECT $1::bigint=(floor((extract(epoch FROM now())-900)/3600)::bigint*3600)"
        ).bind(until_epoch).fetch_one(&mut **tx).await?;
        if !valid {
            return Err(ExposureError::Invalid);
        }
        let start_ms = (until_epoch - i64::from(days) * 172_800) * 1000;
        let end_ms = until_epoch * 1000;
        let middle_ms = end_ms - i64::from(days) * 86_400_000;
        let row = sqlx::query(r#"
WITH epochs AS MATERIALIZED (
 SELECT started_ms,confirmed_ms,retired,ended_ms
 FROM flow_observation_epochs
 WHERE started_ms<$2+902000 AND (NOT retired OR ended_ms>$1)
 ORDER BY started_ms,epoch LIMIT 4097
), coverage AS (
 SELECT count(*) AS workers,
   range_agg(int8range(greatest(started_ms,$1),least(confirmed_ms,$2+902000),'[)'))
   FILTER(WHERE confirmed_ms>$1 AND confirmed_ms>started_ms) AS covered
 FROM epochs
), flags AS (
 SELECT bucket_ms,bool_or(lost) AS lost,bool_or(failed) AS failed,
   bool_or(uncertain) AS uncertain
 FROM flow_observation_health_hourly
 WHERE bucket_ms >= $1 AND bucket_ms < $2 GROUP BY bucket_ms
), hours AS (
 SELECT h AS bucket_ms,
   coalesce(c.covered @> int8range(h,h+4502000,'[)'),false)
   AND NOT coalesce(f.uncertain,false)
   AND NOT EXISTS(
     SELECT 1 FROM epochs e
     WHERE e.started_ms<h+4502000 AND (NOT e.retired OR e.ended_ms>h)
       AND e.confirmed_ms < CASE WHEN e.retired THEN least(h+4502000,e.ended_ms)
                               ELSE h+4502000 END
   ) AS confirmed,
   coalesce(f.lost,false) AS lost,coalesce(f.failed,false) AS failed
 FROM generate_series($1::bigint,$2::bigint-3600000,3600000::bigint) h
 CROSS JOIN coverage c LEFT JOIN flags f ON f.bucket_ms=h
)
SELECT (SELECT workers<=4096 FROM coverage) AS bounded,
 count(*) FILTER(WHERE bucket_ms>=$3 AND confirmed)::int AS current_confirmed,
 count(*) FILTER(WHERE bucket_ms<$3 AND confirmed)::int AS previous_confirmed,
 count(*) FILTER(WHERE bucket_ms>=$3 AND lost)::int AS current_lost,
 count(*) FILTER(WHERE bucket_ms<$3 AND lost)::int AS previous_lost,
 count(*) FILTER(WHERE bucket_ms>=$3 AND failed)::int AS current_failed,
 count(*) FILTER(WHERE bucket_ms<$3 AND failed)::int AS previous_failed
FROM hours
"#).bind(start_ms).bind(end_ms).bind(middle_ms).fetch_one(&mut **tx).await?;
        if !row.try_get::<bool, _>("bounded")? {
            return Err(ExposureError::Unavailable);
        }
        let period = |prefix: &str| -> Result<ExposureHealthPeriod, ExposureError> {
            Ok(ExposureHealthPeriod {
                confirmed_hours: row.try_get(format!("{prefix}_confirmed").as_str())?,
                loss_hours: row.try_get(format!("{prefix}_lost").as_str())?,
                error_hours: row.try_get(format!("{prefix}_failed").as_str())?,
            })
        };
        Ok(ExposureHealth {
            until_epoch, current: period("current")?, previous: period("previous")?,
        })
    }
}
