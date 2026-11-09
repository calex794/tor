//! Read-only, bounded FLOW hours over the existing RIL-owned aggregate table.
use crate::services::audience::{
    report_from_rows, validate_period, FlowHour, FlowHourlyError, FlowHourlyReport, ReportAnchor,
};
use sqlx::{PgPool, Row};

#[derive(Clone)]
pub struct PgFlowAudience {
    pool: PgPool,
}

impl PgFlowAudience {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Two adjacent intervals of complete UTC hours, never the current hour.
    /// Buckets describe batch persistence; delayed observations may arrive late.
    pub async fn report(&self, days: i32) -> Result<FlowHourlyReport, FlowHourlyError> {
        let hours = validate_period(days)?;
        let mut tx = self.pool.begin().await.map_err(unavailable)?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        sqlx::query("SET LOCAL statement_timeout = '750ms'")
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
        let rows = sqlx::query(REPORT_SQL)
            .bind(hours as i32)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?;
        let anchor = rows.first().ok_or(FlowHourlyError::Unavailable)?;
        let anchor = ReportAnchor {
            measured_at_epoch: anchor.try_get("measured_at_epoch").map_err(unavailable)?,
            measured_at_utc: anchor.try_get("measured_at_utc").map_err(unavailable)?,
            until_epoch: anchor.try_get("until_epoch").map_err(unavailable)?,
            until_utc: anchor.try_get("until_utc").map_err(unavailable)?,
            current_from_utc: anchor.try_get("current_from_utc").map_err(unavailable)?,
            previous_from_utc: anchor.try_get("previous_from_utc").map_err(unavailable)?,
        };
        let mut saved = Vec::with_capacity(rows.len());
        for row in rows {
            if !row
                .try_get::<Option<bool>, _>("valid")
                .map_err(unavailable)?
                .unwrap_or(true)
            {
                return Err(FlowHourlyError::Unavailable);
            }
            saved.push(FlowHour {
                hour_epoch: row.try_get("hour_epoch").map_err(unavailable)?,
                hour_utc: row.try_get("hour_utc").map_err(unavailable)?,
                views: [
                    row.try_get("views_0").map_err(unavailable)?,
                    row.try_get("views_1").map_err(unavailable)?,
                    row.try_get("views_2").map_err(unavailable)?,
                    row.try_get("views_3").map_err(unavailable)?,
                ],
            });
        }
        let report = report_from_rows(days, anchor, saved)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(report)
    }
}

fn unavailable(_: sqlx::Error) -> FlowHourlyError {
    FlowHourlyError::Unavailable
}

// No LIMIT: SUM sees every saved source row in the full bounded interval.
// Source and surface cardinality come from the existing constrained schema.
// The bounded hour series retains gaps, without fabricating a zero count.
const REPORT_SQL: &str = r#"
WITH anchor AS MATERIALIZED (
    SELECT statement_timestamp() AS at,
           date_trunc('hour', statement_timestamp() AT TIME ZONE 'UTC')
               AT TIME ZONE 'UTC' AS until_at
), hours AS (
    SELECT generate_series(a.until_at - ($1::integer * 2 * interval '1 hour'),
        a.until_at - interval '1 hour', interval '1 hour') AS bucket
    FROM anchor a
), saved AS (
    SELECT f.bucket, f.surface_code, SUM(f.html_requests)::bigint AS views,
           bool_and(f.acquisition_hint BETWEEN 0 AND 3
               AND f.html_requests >= 0
               AND f.opted_in_html_requests BETWEEN 0 AND f.html_requests
               AND f.observed_opted_in_visit_starts BETWEEN 0 AND f.opted_in_html_requests
               AND f.bucket = date_trunc('hour', f.bucket AT TIME ZONE 'UTC')
                   AT TIME ZONE 'UTC') AS valid
    FROM ril_flow_hourly f CROSS JOIN anchor a
    WHERE f.bucket >= a.until_at - ($1::integer * 2 * interval '1 hour')
      AND f.bucket < a.until_at AND f.surface_code BETWEEN 0 AND 3
    GROUP BY f.bucket, f.surface_code
), validity AS (
    SELECT bool_and(valid) AS valid FROM saved
)
SELECT FLOOR(EXTRACT(EPOCH FROM a.at))::bigint AS measured_at_epoch,
       to_char(a.at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"') AS measured_at_utc,
       EXTRACT(EPOCH FROM a.until_at)::bigint AS until_epoch,
       to_char(a.until_at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"') AS until_utc,
       to_char((a.until_at - ($1::integer * interval '1 hour')) AT TIME ZONE 'UTC',
           'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS current_from_utc,
       to_char((a.until_at - ($1::integer * 2 * interval '1 hour')) AT TIME ZONE 'UTC',
           'YYYY-MM-DD"T"HH24:MI:SS"Z"') AS previous_from_utc,
       EXTRACT(EPOCH FROM h.bucket)::bigint AS hour_epoch,
       to_char(h.bucket AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS"Z"') AS hour_utc,
       MAX(s.views) FILTER (WHERE s.surface_code=0) AS views_0,
       MAX(s.views) FILTER (WHERE s.surface_code=1) AS views_1,
       MAX(s.views) FILTER (WHERE s.surface_code=2) AS views_2,
       MAX(s.views) FILTER (WHERE s.surface_code=3) AS views_3,
       v.valid
FROM anchor a CROSS JOIN hours h CROSS JOIN validity v
LEFT JOIN saved s ON s.bucket=h.bucket
GROUP BY a.at,a.until_at,h.bucket,v.valid
ORDER BY h.bucket
"#;
