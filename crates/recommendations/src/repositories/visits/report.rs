use super::{unavailable, PgFlowVisits};
use crate::services::visits::{
    FlowVisitError, FlowVisitsPeriod, FlowVisitsReport, FlowVisitsRow, FLOW_VISIT_DAYS,
};
use sqlx::Row;

impl PgFlowVisits {
    /// One read-only snapshot, bounded finite dimensions, no identity output.
    /// Daily archived totals and not-yet-finalized visits never overlap.
    /// Archived totals do not prove that identity or continuation receipts remain.
    pub async fn report(&self, days: i32) -> Result<FlowVisitsReport, FlowVisitError> {
        if !FLOW_VISIT_DAYS.contains(&days) {
            return Err(FlowVisitError::Invalid);
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
        let rows = sqlx::query(PERIODS_SQL)
            .bind(days)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?;
        if rows.len() != 2 {
            return Err(FlowVisitError::Unavailable);
        }
        let measured_at_utc = rows[0].try_get("measured_at_utc").map_err(unavailable)?;
        let mut periods = Vec::with_capacity(2);
        for row in rows {
            periods.push(FlowVisitsPeriod {
                from_utc: row.try_get("from_utc").map_err(unavailable)?,
                until_utc: row.try_get("until_utc").map_err(unavailable)?,
                unique_subjects: row.try_get("unique_subjects").map_err(unavailable)?,
                visits: None,
                pageviews: None,
                observed_seconds: None,
                unattributed_pageviews: row
                    .try_get("unattributed_pageviews")
                    .map_err(unavailable)?,
                rows: Vec::new(),
            });
        }
        for row in sqlx::query(ROWS_SQL)
            .bind(days)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?
        {
            let ordinal: i32 = row.try_get("ordinal").map_err(unavailable)?;
            let period = periods
                .get_mut(usize::try_from(ordinal).map_err(|_| FlowVisitError::Unavailable)?)
                .ok_or(FlowVisitError::Unavailable)?;
            let item = FlowVisitsRow {
                acquisition_hint: row.try_get("acquisition_hint").map_err(unavailable)?,
                landing_surface: row.try_get("landing_surface").map_err(unavailable)?,
                last_surface: row.try_get("last_surface").map_err(unavailable)?,
                visits: row.try_get("visits").map_err(unavailable)?,
                pageviews: row.try_get("pageviews").map_err(unavailable)?,
                observed_seconds: row.try_get("observed_seconds").map_err(unavailable)?,
            };
            if !(0..=3).contains(&item.acquisition_hint)
                || !(0..=4).contains(&item.landing_surface)
                || !(0..=4).contains(&item.last_surface)
                || item.visits < 0
                || item.pageviews < item.visits
                || item.observed_seconds < 0
            {
                return Err(FlowVisitError::Unavailable);
            }
            add(&mut period.visits, item.visits)?;
            add(&mut period.pageviews, item.pageviews)?;
            add(&mut period.observed_seconds, item.observed_seconds)?;
            period.rows.push(item);
        }
        let previous = periods.pop().ok_or(FlowVisitError::Unavailable)?;
        let current = periods.pop().ok_or(FlowVisitError::Unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(FlowVisitsReport {
            days,
            measured_at_utc,
            current,
            previous,
        })
    }
}

fn add(total: &mut Option<i64>, value: i64) -> Result<(), FlowVisitError> {
    *total = Some(
        total
            .unwrap_or(0)
            .checked_add(value)
            .ok_or(FlowVisitError::Unavailable)?,
    );
    Ok(())
}

const PERIODS_SQL: &str = r#"
WITH anchor AS (
  SELECT transaction_timestamp() AS at,
         (transaction_timestamp() AT TIME ZONE 'UTC')::date AS today
), periods AS (
  SELECT 0 AS ordinal,today+1-$1::int AS from_day,today+1 AS until_day FROM anchor
  UNION ALL
  SELECT 1,today+1-$1::int*2,today+1-$1::int FROM anchor
)
SELECT p.ordinal,
       to_char(a.at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS measured_at_utc,
       to_char(p.from_day,'YYYY-MM-DD"T"00:00:00"Z"') AS from_utc,
       to_char(p.until_day,'YYYY-MM-DD"T"00:00:00"Z"') AS until_utc,
       CASE WHEN p.from_day>=a.today-29 AND
         EXISTS(SELECT 1 FROM flow_visit_account_days d
                WHERE d.bucket_date>=p.from_day AND d.bucket_date<p.until_day)
       THEN (SELECT count(DISTINCT d.subject_id) FROM flow_visit_account_days d
             WHERE d.bucket_date>=p.from_day AND d.bucket_date<p.until_day)
       ELSE NULL END AS unique_subjects,
       CASE WHEN p.from_day>=a.today-29 AND
         EXISTS(SELECT 1 FROM flow_visit_receipts r
                WHERE r.observed_at>=p.from_day::timestamp AT TIME ZONE 'UTC'
                  AND r.observed_at<p.until_day::timestamp AT TIME ZONE 'UTC')
       THEN (SELECT count(*) FROM flow_visit_receipts r
             WHERE r.continuation_unknown
               AND r.observed_at>=p.from_day::timestamp AT TIME ZONE 'UTC'
               AND r.observed_at<p.until_day::timestamp AT TIME ZONE 'UTC')
       ELSE NULL END AS unattributed_pageviews
FROM periods p CROSS JOIN anchor a ORDER BY p.ordinal
"#;

const ROWS_SQL: &str = r#"
WITH anchor AS (
  SELECT (transaction_timestamp() AT TIME ZONE 'UTC')::date AS today
), periods AS (
  SELECT 0 AS ordinal,today+1-$1::int AS from_day,today+1 AS until_day FROM anchor
  UNION ALL
  SELECT 1,today+1-$1::int*2,today+1-$1::int FROM anchor
), facts AS (
  SELECT cohort_day,acquisition_hint,landing_surface,last_surface,
         visits,pageviews,observed_seconds FROM flow_visit_daily CROSS JOIN anchor
  WHERE cohort_day>=today+1-$1::int*2 AND cohort_day<today+1
  UNION ALL
  SELECT (started_at AT TIME ZONE 'UTC')::date,acquisition_hint,landing_surface,last_surface,
         1::bigint,pageviews,floor(extract(epoch FROM(last_at-started_at)))::bigint
  FROM flow_visits CROSS JOIN anchor
  WHERE finalized_at IS NULL
    AND started_at>=(today+1-$1::int*2)::timestamp AT TIME ZONE 'UTC'
    AND started_at<(today+1)::timestamp AT TIME ZONE 'UTC'
)
SELECT p.ordinal,f.acquisition_hint,f.landing_surface,f.last_surface,
       sum(f.visits)::bigint AS visits,sum(f.pageviews)::bigint AS pageviews,
       sum(f.observed_seconds)::bigint AS observed_seconds
FROM periods p JOIN facts f ON f.cohort_day>=p.from_day AND f.cohort_day<p.until_day
GROUP BY p.ordinal,f.acquisition_hint,f.landing_surface,f.last_surface
ORDER BY p.ordinal,f.acquisition_hint,f.landing_surface,f.last_surface
"#;
