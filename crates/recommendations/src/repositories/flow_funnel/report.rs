use super::{unavailable, PgFlowFunnel};
use crate::services::flow_funnel::{
    FlowFunnelCounts, FlowFunnelError, FlowFunnelPeriod, FlowFunnelReport, FlowFunnelRow,
};
use crate::services::visits::FLOW_VISIT_DAYS;
use sqlx::{postgres::PgRow, Row};

impl PgFlowFunnel {
    /// Same UTC calendar cohorts in both periods. A missing observation is not
    /// zero and an archived daily total never manufactures a unique-person count.
    pub async fn report(&self, days: i32) -> Result<FlowFunnelReport, FlowFunnelError> {
        if !FLOW_VISIT_DAYS.contains(&days) {
            return Err(FlowFunnelError::Invalid);
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
        let periods_rows = sqlx::query(PERIODS_SQL)
            .bind(days)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?;
        if periods_rows.len() != 2 {
            return Err(FlowFunnelError::Unavailable);
        }
        let measured_at_utc = periods_rows[0]
            .try_get("measured_at_utc")
            .map_err(unavailable)?;
        let mut periods = Vec::with_capacity(2);
        for row in periods_rows {
            periods.push(FlowFunnelPeriod {
                from_utc: row.try_get("from_utc").map_err(unavailable)?,
                until_utc: row.try_get("until_utc").map_err(unavailable)?,
                counts: None,
                pending_book_observations: row.try_get("pending").map_err(unavailable)?,
                unmatched_book_observations: row.try_get("unmatched").map_err(unavailable)?,
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
                .get_mut(usize::try_from(ordinal).map_err(|_| FlowFunnelError::Unavailable)?)
                .ok_or(FlowFunnelError::Unavailable)?;
            let item = FlowFunnelRow {
                acquisition_hint: row.try_get("acquisition_hint").map_err(unavailable)?,
                discovery_kind: row.try_get("discovery_kind").map_err(unavailable)?,
                counts: counts(&row)?,
            };
            if ![0, 1, 3].contains(&item.acquisition_hint)
                || !(0..=3).contains(&item.discovery_kind)
            {
                return Err(FlowFunnelError::Unavailable);
            }
            add(
                period.counts.get_or_insert_with(FlowFunnelCounts::default),
                &item.counts,
            )?;
            period.rows.push(item);
        }
        let health = sqlx::query(
            "SELECT to_char(last_success AT TIME ZONE 'UTC','YYYY-MM-DD\"T\"HH24:MI:SS.US\"Z\"') AS last_success,
                    last_collecting,backlog FROM flow_book_worker_health WHERE singleton",
        ).fetch_optional(&mut *tx).await.map_err(unavailable)?;
        let (worker_last_success_utc, worker_collecting, worker_backlog) = match health {
            Some(row) => (
                row.try_get("last_success").map_err(unavailable)?,
                row.try_get("last_collecting").map_err(unavailable)?,
                row.try_get("backlog").map_err(unavailable)?,
            ),
            None => (None, false, true),
        };
        let previous = periods.pop().ok_or(FlowFunnelError::Unavailable)?;
        let current = periods.pop().ok_or(FlowFunnelError::Unavailable)?;
        tx.commit().await.map_err(unavailable)?;
        Ok(FlowFunnelReport {
            days,
            measured_at_utc,
            current,
            previous,
            worker_last_success_utc,
            worker_collecting,
            worker_backlog,
        })
    }
}

fn counts(row: &PgRow) -> Result<FlowFunnelCounts, FlowFunnelError> {
    let counts = FlowFunnelCounts {
        cohorts: row.try_get("cohorts").map_err(unavailable)?,
        detail_opens: row.try_get("detail_opens").map_err(unavailable)?,
        reader_opens: row.try_get("reader_opens").map_err(unavailable)?,
        starts: row.try_get("starts").map_err(unavailable)?,
        completions: row.try_get("completions").map_err(unavailable)?,
        saves: row.try_get("saves").map_err(unavailable)?,
        follows: row.try_get("follows").map_err(unavailable)?,
        new_readers: row.try_get("new_readers").map_err(unavailable)?,
        d7_anchors: row.try_get("d7_anchors").map_err(unavailable)?,
        d7_pending: row.try_get("d7_pending").map_err(unavailable)?,
        d7_returned: row.try_get("d7_returned").map_err(unavailable)?,
        d7_unknown: row.try_get("d7_unknown").map_err(unavailable)?,
    };
    let all = [
        counts.cohorts,
        counts.detail_opens,
        counts.reader_opens,
        counts.starts,
        counts.completions,
        counts.saves,
        counts.follows,
        counts.new_readers,
        counts.d7_anchors,
        counts.d7_pending,
        counts.d7_returned,
        counts.d7_unknown,
    ];
    if all.iter().any(|n| *n < 0)
        || [
            counts.detail_opens,
            counts.reader_opens,
            counts.starts,
            counts.saves,
            counts.follows,
        ]
        .iter()
        .any(|n| *n > counts.cohorts)
        || [counts.completions, counts.new_readers, counts.d7_anchors]
            .iter()
            .any(|n| *n > counts.starts)
        || counts
            .d7_pending
            .checked_add(counts.d7_returned)
            .and_then(|v| v.checked_add(counts.d7_unknown))
            != Some(counts.d7_anchors)
    {
        return Err(FlowFunnelError::Unavailable);
    }
    Ok(counts)
}

fn add(total: &mut FlowFunnelCounts, item: &FlowFunnelCounts) -> Result<(), FlowFunnelError> {
    macro_rules! fields { ($($field:ident),+ $(,)?) => { $(
        total.$field = total.$field.checked_add(item.$field).ok_or(FlowFunnelError::Unavailable)?;
    )+ }; }
    fields!(
        cohorts,
        detail_opens,
        reader_opens,
        starts,
        completions,
        saves,
        follows,
        new_readers,
        d7_anchors,
        d7_pending,
        d7_returned,
        d7_unknown
    );
    Ok(())
}

const PERIODS_SQL: &str = r#"
WITH anchor AS (
 SELECT transaction_timestamp() AS at,(transaction_timestamp() AT TIME ZONE 'UTC')::date AS today
), periods AS (
 SELECT 0 AS ordinal,today+1-$1::int AS from_day,today+1 AS until_day FROM anchor
 UNION ALL SELECT 1,today+1-$1::int*2,today+1-$1::int FROM anchor
), diagnostics AS (
 SELECT p.ordinal,count(*) FILTER(WHERE NOT r.funnel_processed) AS pending,
        count(*) FILTER(WHERE r.funnel_processed AND r.cohort_id IS NULL) AS unmatched
 FROM periods p JOIN flow_visit_receipts r ON r.observed_at>=p.from_day::timestamp AT TIME ZONE 'UTC'
   AND r.observed_at<p.until_day::timestamp AT TIME ZONE 'UTC'
 WHERE r.book_id IS NOT NULL
 GROUP BY p.ordinal
)
SELECT p.ordinal,
 to_char(a.at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS measured_at_utc,
 to_char(p.from_day,'YYYY-MM-DD"T"00:00:00"Z"') AS from_utc,
 to_char(p.until_day,'YYYY-MM-DD"T"00:00:00"Z"') AS until_utc,
 CASE WHEN p.from_day>=a.today-29 THEN d.pending ELSE NULL END AS pending,
 CASE WHEN p.from_day>=a.today-29 THEN d.unmatched ELSE NULL END AS unmatched
FROM periods p CROSS JOIN anchor a LEFT JOIN diagnostics d ON d.ordinal=p.ordinal ORDER BY p.ordinal
"#;

const ROWS_SQL: &str = r#"
WITH anchor AS (SELECT (transaction_timestamp() AT TIME ZONE 'UTC')::date AS today), periods AS (
 SELECT 0 AS ordinal,today+1-$1::int AS from_day,today+1 AS until_day FROM anchor
 UNION ALL SELECT 1,today+1-$1::int*2,today+1-$1::int FROM anchor
), facts AS (
 SELECT cohort_day,book_id,acquisition_hint,discovery_kind,
   cohorts,detail_opens,reader_opens,starts,completions,saves,follows,new_readers,
   d7_anchors,0::bigint AS d7_pending,d7_returned,d7_unknown
 FROM flow_book_daily CROSS JOIN anchor WHERE cohort_day>=today+1-$1::int*2 AND cohort_day<today+1
 UNION ALL
 SELECT (first_entry_at AT TIME ZONE 'UTC')::date,book_id,acquisition_hint,discovery_kind,
   1::bigint,has_detail::int,has_reader::int,started::int,completed::int,saved::int,followed::int,new_reader::int,
   (d7_state>0)::int,(d7_state=1)::int,(d7_state=2)::int,(d7_state=3)::int
 FROM flow_book_cohorts CROSS JOIN anchor WHERE finalized_at IS NULL
   AND first_entry_at>=(today+1-$1::int*2)::timestamp AT TIME ZONE 'UTC'
   AND first_entry_at<(today+1)::timestamp AT TIME ZONE 'UTC'
)
SELECT p.ordinal,f.acquisition_hint,f.discovery_kind,
 sum(f.cohorts)::bigint AS cohorts,sum(f.detail_opens)::bigint AS detail_opens,
 sum(f.reader_opens)::bigint AS reader_opens,sum(f.starts)::bigint AS starts,
 sum(f.completions)::bigint AS completions,sum(f.saves)::bigint AS saves,
 sum(f.follows)::bigint AS follows,sum(f.new_readers)::bigint AS new_readers,
 sum(f.d7_anchors)::bigint AS d7_anchors,sum(f.d7_pending)::bigint AS d7_pending,
 sum(f.d7_returned)::bigint AS d7_returned,sum(f.d7_unknown)::bigint AS d7_unknown
FROM periods p JOIN facts f ON f.cohort_day>=p.from_day AND f.cohort_day<p.until_day
GROUP BY p.ordinal,f.acquisition_hint,f.discovery_kind ORDER BY p.ordinal,f.acquisition_hint,f.discovery_kind
"#;
