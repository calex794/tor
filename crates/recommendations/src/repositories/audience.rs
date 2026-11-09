//! Numeric discovery read model for the separately authorized site admin.
//! Reads anonymous retained daily facts; no profile or attribution is created.
use crate::services::audience::{
    validate_period, DiscoveryChannel, DiscoveryError, DiscoveryHealth, DiscoveryPeriod,
    DiscoveryReport, DiscoverySource,
};
use sqlx::{postgres::PgRow, PgPool, Row};

#[derive(Clone)]
pub struct PgDiscoveryAudience {
    pool: PgPool,
}

impl PgDiscoveryAudience {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Two adjacent UTC calendar periods share one read-only snapshot.
    /// SQL aggregates the complete retained range before returning eight rows.
    pub async fn report(&self, days: i32) -> Result<DiscoveryReport, DiscoveryError> {
        validate_period(days)?;
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
            .bind(days)
            .fetch_all(&mut *tx)
            .await
            .map_err(unavailable)?;
        if rows.len() != 8 {
            return Err(DiscoveryError::Unavailable);
        }
        let measured_at_utc = rows[0].try_get("measured_at").map_err(unavailable)?;
        let current = period(&rows[..4])?;
        let previous = period(&rows[4..])?;
        let health = DiscoveryHealth {
            last_success_utc: rows[0].try_get("last_success").map_err(unavailable)?,
            fresh: rows[0].try_get("fresh").map_err(unavailable)?,
            rec_dirty: rows[0].try_get("rec_dirty").map_err(unavailable)?,
            organic_dirty: rows[0].try_get("organic_dirty").map_err(unavailable)?,
        };
        tx.commit().await.map_err(unavailable)?;
        Ok(DiscoveryReport {
            days,
            measured_at_utc,
            current,
            previous,
            health,
        })
    }
}

fn unavailable(_: sqlx::Error) -> DiscoveryError {
    DiscoveryError::Unavailable
}

fn period(rows: &[PgRow]) -> Result<DiscoveryPeriod, DiscoveryError> {
    let channels = rows
        .iter()
        .map(|row| {
            let source: &str = row.try_get("source").map_err(unavailable)?;
            let source = match source {
                "catalog" => DiscoverySource::Catalog,
                "search" => DiscoverySource::Search,
                "recommendations" => DiscoverySource::Recommendations,
                "unattributed" => DiscoverySource::Unattributed,
                _ => return Err(DiscoveryError::Unavailable),
            };
            Ok(DiscoveryChannel {
                source,
                clicks: row.try_get("clicks").map_err(unavailable)?,
                book_opens: row.try_get("book_opens").map_err(unavailable)?,
                starts: row.try_get("starts").map_err(unavailable)?,
                completions: row.try_get("completions").map_err(unavailable)?,
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(DiscoveryPeriod {
        from_day: rows[0].try_get("from_day").map_err(unavailable)?,
        until_day_exclusive: rows[0].try_get("until_day").map_err(unavailable)?,
        includes_partial_today: rows[0].try_get("partial_today").map_err(unavailable)?,
        channels,
    })
}

// No join to today's public-book projection: a later hide does not rewrite a
// previously authorized anonymous fact, and no book/user identifier leaves SQL.
// A NULL REC book-open bucket predates reliable measurement; partial SUMs must
// not be presented as a complete historical count. No raw 30-day rows are used.
const REPORT_SQL: &str = r#"
WITH anchor AS (
    SELECT transaction_timestamp() AS at,
           (transaction_timestamp() AT TIME ZONE 'UTC')::date+1 AS until_day
), periods AS (
    SELECT 0 AS ordinal, until_day-$1::int AS from_day, until_day, at FROM anchor
    UNION ALL
    SELECT 1, until_day-$1::int*2, until_day-$1::int, at FROM anchor
), channels(channel_order,source) AS (
    VALUES (0,'catalog'),(1,'search'),(2,'recommendations'),(3,'unattributed')
), totals AS (
    SELECT CASE WHEN q.bucket_date>=a.until_day-$1::int THEN 0 ELSE 1 END AS ordinal,
           'recommendations' AS source,
           sum(q.paired_clicks+q.unpaired_clicks)::bigint AS clicks,
           CASE WHEN count(q.book_opens)=count(*)
                THEN sum(q.book_opens)::bigint END AS book_opens,
           sum(q.starts)::bigint AS starts, sum(q.completions)::bigint AS completions
    FROM rec_quality_daily q CROSS JOIN anchor a
    WHERE q.bucket_date>=a.until_day-$1::int*2 AND q.bucket_date<a.until_day
    GROUP BY 1
    UNION ALL
    SELECT CASE WHEN q.bucket_date>=a.until_day-$1::int THEN 0 ELSE 1 END,
           q.source,
           CASE WHEN q.source<>'unattributed' THEN sum(q.clicks)::bigint END,
           CASE WHEN q.source<>'unattributed' THEN sum(q.book_opens)::bigint END,
           sum(q.starts)::bigint, sum(q.completions)::bigint
    FROM organic_discovery_quality_daily q CROSS JOIN anchor a
    WHERE q.bucket_date>=a.until_day-$1::int*2 AND q.bucket_date<a.until_day
      AND q.source IN ('catalog','search','unattributed')
    GROUP BY 1,q.source
), health AS (
    SELECT (SELECT last_success FROM rec_worker_health WHERE singleton) AS last_success,
           EXISTS(SELECT 1 FROM rec_dirty_buckets) AS rec_dirty,
           EXISTS(SELECT 1 FROM organic_discovery_dirty) AS organic_dirty
)
SELECT p.ordinal,c.source,
       to_char(p.at AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS measured_at,
       to_char(p.from_day,'YYYY-MM-DD') AS from_day,
       to_char(p.until_day,'YYYY-MM-DD') AS until_day,
       p.ordinal=0 AS partial_today,
       t.clicks,t.book_opens,t.starts,t.completions,
       to_char(h.last_success AT TIME ZONE 'UTC','YYYY-MM-DD"T"HH24:MI:SS.US"Z"') AS last_success,
       COALESCE(h.last_success>p.at-interval '30 seconds' AND h.last_success<=p.at,false) AS fresh,
       h.rec_dirty,h.organic_dirty
FROM periods p CROSS JOIN channels c CROSS JOIN health h
LEFT JOIN totals t ON t.ordinal=p.ordinal AND t.source=c.source
ORDER BY p.ordinal,c.channel_order
"#;
